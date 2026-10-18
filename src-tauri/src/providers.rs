use futures_util::StreamExt;
use reqwest::{Client, Request, Response, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    OpenAi,    // /chat/completions
    Anthropic, // /messages
    Gemini,    // :generateContent
    Ollama,    // /api/generate
}

pub struct ProviderDef {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    pub base_url: &'static str,
    pub default_model: &'static str,
    pub known_models: &'static [&'static str],
    pub env_keys: &'static [&'static str],
    pub key_url: &'static str,
    pub requires_key: bool,
}

pub static PROVIDERS: &[ProviderDef] = &[
    ProviderDef {
        id: "openai",
        name: "OpenAI",
        kind: Kind::OpenAi,
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-4o-mini",
        known_models: &["gpt-4o-mini", "gpt-4o", "o4-mini"],
        env_keys: &["OPENAI_API_KEY"],
        key_url: "https://platform.openai.com/api-keys",
        requires_key: true,
    },
    ProviderDef {
        id: "gemini",
        name: "Gemini",
        kind: Kind::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        default_model: "gemini-2.0-flash",
        known_models: &[
            "gemini-2.0-flash",
            "gemini-2.0-flash-lite",
            "gemini-1.5-pro",
            "gemini-2.5-flash",
        ],
        env_keys: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        key_url: "https://aistudio.google.com/apikey",
        requires_key: true,
    },
    ProviderDef {
        id: "anthropic",
        name: "Anthropic",
        kind: Kind::Anthropic,
        base_url: "https://api.anthropic.com/v1",
        default_model: "claude-sonnet-4-5",
        known_models: &[
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
            "claude-opus-4-1",
        ],
        env_keys: &["ANTHROPIC_API_KEY"],
        key_url: "https://console.anthropic.com/settings/keys",
        requires_key: true,
    },
];

pub fn find_provider(id: &str) -> Option<&'static ProviderDef> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// Resolved provider: preset or custom (custom = OpenAI-compatible).
pub struct Resolved {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: Option<f32>,
}

pub fn strip_think_blocks(text: &str) -> String {
    // Remove  reasoning blocks (may be unclosed while streaming).
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

pub fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

pub fn http_error(provider: &str, status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            let msg = v
                .get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(|m| m.as_str())
                        .map(String::from)
                        .or_else(|| e.as_str().map(String::from))
                })
                .or_else(|| v.get("message").and_then(|m| m.as_str()).map(String::from));
            msg
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    if msg.is_empty() {
        format!("{provider} HTTP {status}")
    } else {
        format!("{provider} HTTP {status}: {msg}")
    }
}

const MAX_ATTEMPTS: usize = 3;
const MAX_BACKOFF_SECS: u64 = 30;

fn retryable_status(status: StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

fn parse_retry_after(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok().map(|s| s.min(MAX_BACKOFF_SECS))
}

/// Send with backoff on 429/5xx and connect errors, honoring the `Retry-After`
/// seconds form. Returns the final response; the caller formats the error from
/// its body. Only a non-retryable status or the last attempt exits early.
async fn send_with_retry(http: &Client, req: &Request, name: &str) -> Result<Response, String> {
    let mut delay = 1u64;
    for attempt in 1..=MAX_ATTEMPTS {
        let cloned = match req.try_clone() {
            Some(c) => c,
            None => return Err(format!("{name}: cannot clone request")),
        };
        match http.execute(cloned).await {
            Ok(resp) => {
                if !retryable_status(resp.status()) || attempt == MAX_ATTEMPTS {
                    return Ok(resp);
                }
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_retry_after)
                    .unwrap_or(delay);
                log::warn!(
                    "{name} HTTP {} — retry in {wait}s (attempt {attempt}/{MAX_ATTEMPTS})",
                    resp.status().as_u16()
                );
                sleep(Duration::from_secs(wait)).await;
                delay = (delay * 2).min(MAX_BACKOFF_SECS);
            }
            Err(e) => {
                if attempt == MAX_ATTEMPTS || !e.is_connect() {
                    return Err(format!("{name} request failed: {e}"));
                }
                log::warn!(
                    "{name} connect error — retry in {delay}s (attempt {attempt}/{MAX_ATTEMPTS})"
                );
                sleep(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(MAX_BACKOFF_SECS);
            }
        }
    }
    Err(format!("{name}: retry loop exhausted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_think_blocks_closed() {
        assert_eq!(strip_think_blocks("<think>plan
</think>

Result"), "Result");
    }

    #[test]
    fn strip_think_blocks_unclosed_drops_tail() {
        let streamed = "Result\n\n".to_owned() + "<think>" + "\nhalf-streamed";
        assert_eq!(strip_think_blocks(&streamed), "Result");
    }

    #[test]
    fn http_error_parses_openai_shape() {
        let e = http_error("openai", 429, r#"{"error":{"message":"rate limited"}}"#);
        assert_eq!(e, "openai HTTP 429: rate limited");
    }

    #[test]
    fn http_error_string_error_shape() {
        let e = http_error("anthropic", 401, r#"{"error":"invalid key"}"#);
        assert_eq!(e, "anthropic HTTP 401: invalid key");
    }

    #[test]
    fn http_error_non_json_body_truncates() {
        let e = http_error("gemini", 500, "Internal Server Error");
        assert_eq!(e, "gemini HTTP 500: Internal Server Error");
    }

    #[test]
    fn provider_lookup() {
        assert_eq!(find_provider("openai").map(|p| p.kind), Some(Kind::OpenAi));
        assert_eq!(find_provider("anthropic").map(|p| p.kind), Some(Kind::Anthropic));
        assert!(find_provider("ollama").is_none()); // not a preset; custom providers cover it
        assert!(find_provider("nope").is_none());
    }

    #[test]
    fn retryable_statuses() {
        assert!(retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(retryable_status(StatusCode::BAD_GATEWAY));
        assert!(!retryable_status(StatusCode::BAD_REQUEST));
        assert!(!retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!retryable_status(StatusCode::OK));
    }

    #[test]
    fn retry_after_seconds_capped() {
        assert_eq!(parse_retry_after("7"), Some(7));
        assert_eq!(parse_retry_after(" 12 "), Some(12));
        assert_eq!(parse_retry_after("9999"), Some(MAX_BACKOFF_SECS));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2025 07:28:00 GMT"), None); // date form not parsed
    }

    #[tokio::test]
    async fn retry_loop_recovers_after_two_429s() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            for i in 0..3 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await.unwrap_or(0); // drain request headers
                let resp = if i < 2 {
                    "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                };
                sock.write_all(resp.as_bytes()).await.unwrap();
            }
        });

        let url = reqwest::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let req = Request::new(reqwest::Method::POST, url);
        let resp = send_with_retry(&client(), &req, "mock").await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "ok");
        server.await.unwrap();
    }
}

#[allow(dead_code)]
pub async fn generate(resolved: &Resolved, prompt: &str) -> Result<String, String> {
    generate_stream(resolved, prompt, |_| {}).await
}

pub async fn generate_stream<F>(
    resolved: &Resolved,
    prompt: &str,
    mut on_chunk: F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    if resolved.model.trim().is_empty() {
        return Err(format!("No model selected for {}", resolved.name));
    }
    if resolved.api_key.trim().is_empty() && resolved.kind != Kind::Ollama {
        return Err(format!(
            "Set an API key for {} in Settings (or its env var)",
            resolved.name
        ));
    }
    let http = client();
    let raw = match resolved.kind {
        Kind::OpenAi => generate_openai_stream(&http, resolved, prompt, &mut on_chunk).await?,
        Kind::Anthropic => generate_anthropic_stream(&http, resolved, prompt, &mut on_chunk).await?,
        Kind::Gemini => generate_gemini_stream(&http, resolved, prompt, &mut on_chunk).await?,
        Kind::Ollama => generate_ollama_stream(&http, resolved, prompt, &mut on_chunk).await?,
    };
    Ok(strip_think_blocks(&raw))
}

async fn generate_openai_stream<F>(
    http: &Client,
    r: &Resolved,
    prompt: &str,
    on_chunk: &mut F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    let mut body = json!({
        "model": r.model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": true,
    });
    if let Some(t) = r.temperature {
        body["temperature"] = json!(t);
    }
    let req = http
        .post(format!("{}/chat/completions", r.base_url))
        .bearer_auth(&r.api_key)
        .json(&body)
        .build()
        .map_err(|e| format!("{} request failed: {}", r.name, e))?;
    let resp = send_with_retry(http, &req, &r.name).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(http_error(&r.name, status.as_u16(), &text));
    }

    let mut stream = resp.bytes_stream();
    let mut full_text = String::new();
    let mut line_buf = String::new();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Stream error: {e}"))?;
        let s = String::from_utf8_lossy(&chunk);
        line_buf.push_str(&s);

        while let Some(pos) = line_buf.find('\n') {
            let line = line_buf[..pos].trim().to_string();
            line_buf = line_buf[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if let Some(delta) = v.get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                        .and_then(|d| d.get("content"))
                        .and_then(|s| s.as_str())
                    {
                        if !delta.is_empty() {
                            full_text.push_str(delta);
                            on_chunk(delta);
                        }
                    }
                }
            }
        }
    }

    Ok(full_text)
}

async fn generate_anthropic_stream<F>(
    http: &Client,
    r: &Resolved,
    prompt: &str,
    on_chunk: &mut F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    let mut body = json!({
        "model": r.model,
        "max_tokens": 8192,
        "messages": [{"role": "user", "content": prompt}],
        "stream": true,
    });
    if let Some(t) = r.temperature {
        body["temperature"] = json!(t);
    }
    let req = http
        .post(format!("{}/messages", r.base_url))
        .header("x-api-key", &r.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .build()
        .map_err(|e| format!("{} request failed: {}", r.name, e))?;
    let resp = send_with_retry(http, &req, &r.name).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(http_error(&r.name, status.as_u16(), &text));
    }

    let mut stream = resp.bytes_stream();
    let mut full_text = String::new();
    let mut line_buf = String::new();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Stream error: {e}"))?;
        let s = String::from_utf8_lossy(&chunk);
        line_buf.push_str(&s);

        while let Some(pos) = line_buf.find('\n') {
            let line = line_buf[..pos].trim().to_string();
            line_buf = line_buf[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    let ev_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if ev_type == "content_block_delta" {
                        if let Some(delta) = v.get("delta")
                            .and_then(|d| d.get("text"))
                            .and_then(|s| s.as_str())
                        {
                            if !delta.is_empty() {
                                full_text.push_str(delta);
                                on_chunk(delta);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(full_text)
}

async fn generate_gemini_stream<F>(
    http: &Client,
    r: &Resolved,
    prompt: &str,
    on_chunk: &mut F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    let mut body = json!({
        "contents": [{"role": "user", "parts": [{"text": prompt}]}],
    });
    if let Some(t) = r.temperature {
        body["generationConfig"] = json!({"temperature": t});
    }
    let req = http
        .post(format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            r.base_url, r.model
        ))
        .header("x-goog-api-key", &r.api_key)
        .json(&body)
        .build()
        .map_err(|e| format!("{} request failed: {}", r.name, e))?;
    let resp = send_with_retry(http, &req, &r.name).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(http_error(&r.name, status.as_u16(), &text));
    }

    let mut stream = resp.bytes_stream();
    let mut full_text = String::new();
    let mut line_buf = String::new();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Stream error: {e}"))?;
        let s = String::from_utf8_lossy(&chunk);
        line_buf.push_str(&s);

        while let Some(pos) = line_buf.find('\n') {
            let line = line_buf[..pos].trim().to_string();
            line_buf = line_buf[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if let Some(parts) = v.get("candidates")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("content"))
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.as_array())
                    {
                        for part in parts {
                            if part.get("thought").and_then(|t| t.as_bool()) != Some(true) {
                                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                    if !txt.is_empty() {
                                        full_text.push_str(txt);
                                        on_chunk(txt);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(full_text)
}

async fn generate_ollama_stream<F>(
    http: &Client,
    r: &Resolved,
    prompt: &str,
    on_chunk: &mut F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    let mut body = json!({"model": r.model, "prompt": prompt, "stream": true});
    if let Some(t) = r.temperature {
        body["options"] = json!({"temperature": t});
    }
    let req = http
        .post(format!("{}/api/generate", r.base_url))
        .json(&body)
        .build()
        .map_err(|e| format!("{} request failed: {}", r.name, e))?;
    let resp = send_with_retry(http, &req, &r.name).await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(http_error(&r.name, status.as_u16(), &text));
    }

    let mut stream = resp.bytes_stream();
    let mut full_text = String::new();
    let mut line_buf = String::new();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Stream error: {e}"))?;
        let s = String::from_utf8_lossy(&chunk);
        line_buf.push_str(&s);

        while let Some(pos) = line_buf.find('\n') {
            let line = line_buf[..pos].trim().to_string();
            line_buf = line_buf[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if let Some(delta) = v.get("response").and_then(|r| r.as_str()) {
                    if !delta.is_empty() {
                        full_text.push_str(delta);
                        on_chunk(delta);
                    }
                }
            }
        }
    }

    Ok(full_text)
}

pub async fn list_models(r: &Resolved) -> Result<Vec<String>, String> {
    let http = client();
    let url = match r.kind {
        Kind::Ollama => format!("{}/api/tags", r.base_url),
        Kind::Gemini => format!("{}/models", r.base_url),
        Kind::Anthropic => format!("{}/models", r.base_url),
        Kind::OpenAi => format!("{}/models", r.base_url),
    };
    let mut req = http.get(&url);
    match r.kind {
        Kind::OpenAi => req = req.bearer_auth(&r.api_key),
        Kind::Anthropic => {
            req = req.header("x-api-key", &r.api_key).header("anthropic-version", "2023-06-01")
        }
        Kind::Gemini => req = req.header("x-goog-api-key", &r.api_key),
        Kind::Ollama => {}
    }
    let req = req.build().map_err(|e| e.to_string())?;
    let resp = send_with_retry(&http, &req, &r.name).await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(http_error(&r.name, status.as_u16(), &text));
    }
    let payload: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let items = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").or_else(|| item.get("name"))?.as_str()?;
            let id = id.strip_prefix("models/").unwrap_or(id);
            if r.kind == Kind::Gemini {
                let ok = item
                    .get("supportedGenerationMethods")
                    .and_then(|m| m.as_array())
                    .map(|m| {
                        m.iter()
                            .any(|x| x.as_str() == Some("generateContent"))
                    })
                    .unwrap_or(false);
                if !ok {
                    return None;
                }
            }
            Some(id.to_string())
        })
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}
