use crate::config::{self, Config, CustomProvider};
use crate::providers::{self, Kind, Resolved};
use std::env;

pub fn resolve(cfg: &Config) -> Resolved {
    if let Some(custom) = cfg
        .custom_providers
        .iter()
        .find(|c| c.id == cfg.provider)
    {
        return Resolved {
            id: custom.id.clone(),
            name: custom.name.clone(),
            kind: Kind::OpenAi,
            base_url: custom.base_url.trim_end_matches('/').to_string(),
            api_key: custom.api_key.clone(),
            model: custom.model.clone(),
            temperature: cfg.temperature,
        };
    }
    if let Some(def) = providers::find_provider(&cfg.provider) {
        let api_key = cfg
            .api_keys
            .get(def.id)
            .cloned()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| {
                def.env_keys
                    .iter()
                    .find_map(|k| env::var(k).ok())
                    .filter(|v| !v.trim().is_empty())
            })
            .unwrap_or_default();
        let model = match def.id {
            "openai" => &cfg.openai_model,
            "gemini" => &cfg.gemini_model,
            "anthropic" => &cfg.anthropic_model,
            _ => "",
        };
        return Resolved {
            id: def.id.to_string(),
            name: def.name.to_string(),
            kind: def.kind,
            base_url: def.base_url.trim_end_matches('/').to_string(),
            api_key,
            model: if model.trim().is_empty() {
                def.default_model.to_string()
            } else {
                model.to_string()
            },
            temperature: cfg.temperature,
        };
    }
    // Fallback: first provider with a key, else openai
    let def = providers::find_provider("openai").unwrap();
    Resolved {
        id: def.id.to_string(),
        name: def.name.to_string(),
        kind: def.kind,
        base_url: def.base_url.to_string(),
        api_key: String::new(),
        model: def.default_model.to_string(),
        temperature: cfg.temperature,
    }
}

pub fn correct_prompt(text: &str) -> String {
    format!(
        "You're a language enhancer expert. Fix grammar, spelling, and clarity while preserving \
         the language and formatting of the input. Return ONLY the corrected text.\n\nText:\n{text}"
    )
}

pub fn translate_prompt(text: &str, lang: &str) -> String {
    format!(
        "You're a translator expert. Translate accurately, preserving formatting. Return ONLY the \
         translated text to {lang}.\n\nText:\n{text}"
    )
}

pub fn add_custom_provider(
    cfg: &mut Config,
    name: String,
    base_url: String,
    api_key: String,
    model: String,
) -> CustomProvider {
    let mut id = config::slugify(&name);
    let mut n = 2;
    while cfg.custom_providers.iter().any(|c| c.id == id) {
        id = format!("{}-{}", config::slugify(&name), n);
        n += 1;
    }
    let provider = CustomProvider {
        id,
        name,
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key,
        model,
    };
    cfg.custom_providers.push(provider.clone());
    provider
}
