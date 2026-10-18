use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub const APP_DIR: &str = "Correctr";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub prompt: String, // template with {text}
    #[serde(default)]
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub provider: String, // "openai" | "gemini" | "anthropic" | custom id
    #[serde(default)]
    pub openai_model: String,
    #[serde(default)]
    pub gemini_model: String,
    #[serde(default)]
    pub anthropic_model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
    #[serde(default = "default_preview_before_replace")]
    pub preview_before_replace: bool,
    #[serde(default)]
    pub auto_paste: bool,
    #[serde(default)]
    pub hotkey: String,
    #[serde(default)]
    pub custom_providers: Vec<CustomProvider>,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub api_keys: HashMap<String, String>, // provider id -> key (600 perms on file)
    #[serde(default)]
    pub models: HashMap<String, Vec<String>>, // provider id -> known/fetched model ids
}

fn default_provider() -> String {
    "openai".into()
}

fn default_languages() -> Vec<String> {
    vec!["French".into(), "English".into(), "Chinese".into(), "Spanish".into()]
}

fn default_preview_before_replace() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            provider: default_provider(),
            openai_model: String::new(),
            gemini_model: String::new(),
            anthropic_model: String::new(),
            temperature: None,
            languages: default_languages(),
            preview_before_replace: true,
            // Replacing the selection is the whole point of the hotkey; leaving
            // the result on the clipboard is a surprise, not a safe default.
            auto_paste: true,
            hotkey: "Ctrl+Cmd+C".into(),
            custom_providers: Vec::new(),
            skills: default_skills(),
            api_keys: HashMap::new(),
            models: HashMap::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
        .join("config.json")
}

pub fn load() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(cfg)?;
    fs::write(&path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn skill(id: &str, name: &str, desc: &str, prompt: &str) -> Skill {
    Skill {
        id: id.into(),
        name: name.into(),
        description: desc.into(),
        prompt: prompt.into(),
        builtin: true,
    }
}

pub fn default_skills() -> Vec<Skill> {
    vec![
        skill(
            "token-saver",
            "Token Saver",
            "Compress text to cut tokens",
            "Compress the following text. Remove filler words, redundant phrases, and pleasantries. \
             Keep all technical terms, names, numbers, and key facts. Output the shorter version only.\n\nText:\n{text}",
        ),
        skill(
            "debug-helper",
            "Debug Helper",
            "Analyze code and suggest fixes",
            "You are a code debugger. Analyze the code below. List each bug or issue as:\n\
             - LINE: issue → fix\n\nIf no bugs found, say \"No issues found.\" Output ONLY the bug list.\n\nCode:\n{text}",
        ),
        skill(
            "explain-code",
            "Explain Code",
            "Explain what code does, concisely",
            "Explain what the following code does, in at most 6 bullet points, no preamble.\n\nCode:\n{text}",
        ),
        skill(
            "commit-message",
            "Git Commit Message",
            "Write a conventional commit message",
            "Write ONE conventional commit message (type(scope): subject, max 72 chars) for the \
             following diff or description. Output only the commit line.\n\nInput:\n{text}",
        ),
        skill(
            "json-cleaner",
            "JSON Cleaner",
            "Reformat and validate JSON",
            "Reformat the JSON below as pretty-printed, valid JSON. Output ONLY the JSON, no \
             markdown fences, no commentary.\n\nJSON:\n{text}",
        ),
    ]
}

#[allow(dead_code)]
pub fn builtin_skills() -> Vec<Skill> {
    default_skills()
}

pub fn fill_template(template: &str, text: &str) -> String {
    template.replace("{text}", text)
}

pub fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "custom".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Gateway"), "my-gateway");
        assert_eq!(slugify("  LM Studio!! "), "lm-studio");
        assert_eq!(slugify("!!!"), "custom");
    }

    #[test]
    fn fill_template_basic() {
        assert_eq!(fill_template("Fix:\n{text}", "helo"), "Fix:\nhelo");
    }

    #[test]
    fn config_roundtrip() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, cfg.provider);
        assert_eq!(back.hotkey, cfg.hotkey);
        assert_eq!(back.languages, cfg.languages);
        assert_eq!(back.skills.len(), cfg.skills.len());
    }

    #[test]
    fn partial_json_falls_back_to_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"provider":"gemini"}"#).unwrap();
        assert_eq!(cfg.provider, "gemini");
        assert_eq!(cfg.languages, default_languages());
        assert!(cfg.preview_before_replace);
    }
}
