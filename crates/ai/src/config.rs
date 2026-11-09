//! What the user configures in Settings → AI, with defaults.

use serde::{Deserialize, Serialize};

/// Providers offered by name. Every one speaks the OpenAI-compatible
/// `/chat/completions` API; `custom` takes any base URL.
#[derive(Clone, Debug, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    /// Whether requests need an API key.
    pub needs_key: bool,
    pub hint: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "fastrouter",
        name: "FastRouter",
        base_url: "https://go.fastrouter.ai/api/v1",
        needs_key: true,
        hint: "Models look like anthropic/claude-sonnet-4.",
    },
    Preset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        needs_key: true,
        hint: "Models look like anthropic/claude-sonnet-4.",
    },
    Preset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        needs_key: true,
        hint: "Models look like gpt-4.1-mini.",
    },
    Preset {
        id: "ollama",
        name: "Ollama (on this computer)",
        base_url: "http://localhost:11434/v1",
        needs_key: false,
        hint: "Runs locally; nothing leaves the device. Models look like llama3.1.",
    },
    Preset {
        id: "custom",
        name: "Custom",
        base_url: "",
        needs_key: true,
        hint: "Any OpenAI-compatible endpoint.",
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// Off: plugins see AI as unavailable.
    pub enabled: bool,
    pub preset: String,
    pub base_url: String,
    /// Default model; a request may name another.
    pub model: String,
    pub temperature: f64,
    /// Upper bound on a reply's length, in tokens.
    pub max_tokens: u32,
    pub timeout_s: u32,
    /// Requests running at once.
    pub concurrency: u8,
    /// Requests allowed per calendar month across all devices; 0 = no limit.
    pub monthly_limit: u32,
    /// Largest prompt (system + user text) accepted, in KB.
    pub max_prompt_kb: u32,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: true,
            preset: "fastrouter".into(),
            base_url: PRESETS[0].base_url.into(),
            model: String::new(),
            temperature: 0.3,
            max_tokens: 2000,
            timeout_s: 120,
            concurrency: 2,
            monthly_limit: 0,
            max_prompt_kb: 200,
        }
    }
}

impl AiConfig {
    /// Clamps numbers into sensible ranges and checks the URL.
    pub fn validate(mut self) -> Result<AiConfig, String> {
        if preset(&self.preset).is_none() {
            return Err(format!("unknown provider `{}`", self.preset));
        }
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        let http = self.base_url.starts_with("https://") || self.base_url.starts_with("http://");
        if !self.base_url.is_empty() && !http {
            return Err("the base URL must start with https:// or http://".into());
        }
        self.model = self.model.trim().to_string();
        if self.model.len() > 200 {
            return Err("the model name is too long".into());
        }
        if !self.temperature.is_finite() {
            self.temperature = 0.3;
        }
        self.temperature = self.temperature.clamp(0.0, 2.0);
        self.max_tokens = self.max_tokens.clamp(100, 32_000);
        self.timeout_s = self.timeout_s.clamp(10, 600);
        self.concurrency = self.concurrency.clamp(1, 8);
        self.max_prompt_kb = self.max_prompt_kb.clamp(8, 2_000);
        Ok(self)
    }

    pub fn needs_key(&self) -> bool {
        preset(&self.preset).is_none_or(|p| p.needs_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_clamps_and_checks() {
        let c = AiConfig {
            temperature: 9.0,
            concurrency: 0,
            base_url: " https://x.dev/v1/ ".into(),
            ..AiConfig::default()
        }
        .validate()
        .unwrap();
        assert_eq!((c.temperature, c.concurrency), (2.0, 1));
        assert_eq!(c.base_url, "https://x.dev/v1");
        assert!(AiConfig {
            base_url: "ftp://x".into(),
            ..AiConfig::default()
        }
        .validate()
        .is_err());
        assert!(!AiConfig {
            preset: "ollama".into(),
            ..AiConfig::default()
        }
        .needs_key());
    }
}
