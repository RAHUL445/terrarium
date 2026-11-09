//! The HTTP side: one blocking OpenAI-compatible chat completion. Hosts call
//! it on a thread they own, never while holding the plugin host.

use std::io::Read;
use std::time::Duration;

const MAX_RESPONSE: usize = 1024 * 1024;

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::AiConfig;

/// A failed request. `code` is stable: `unauthorized`, `rate-limited`,
/// `offline`, `timeout`, `too-large`, `limit`, `not-configured`,
/// `provider` or `invalid`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AiError {
    pub code: String,
    pub message: String,
    /// Worth trying again later without changing anything.
    pub temporary: bool,
}

impl AiError {
    pub fn new(code: &str, message: impl Into<String>) -> AiError {
        AiError {
            code: code.into(),
            message: message.into(),
            temporary: matches!(code, "rate-limited" | "offline" | "timeout" | "provider"),
        }
    }
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Completion {
    pub text: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// What a plugin asks for, already checked against the config.
#[derive(Clone, Debug, Serialize)]
pub struct Prompt {
    pub system: String,
    pub prompt: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f64,
}

fn agent(config: &AiConfig) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(u64::from(config.timeout_s)))
        .build()
}

fn request(config: &AiConfig, key: Option<&str>, method: &str, path: &str) -> ureq::Request {
    let url = format!("{}/{path}", config.base_url);
    let mut req = agent(config)
        .request(method, &url)
        .set("Content-Type", "application/json")
        .set("X-Title", "Terrarium");
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }
    req
}

fn map_error(e: ureq::Error) -> AiError {
    match e {
        ureq::Error::Status(status, _response) => {
            // Provider error bodies can echo prompts, credentials and personal
            // data. Do not read or persist them in job errors/notifications.
            let detail = format!("HTTP {status}");
            let code = match status {
                401 | 403 => "unauthorized",
                404 => "invalid",
                408 | 504 => "timeout",
                413 => "too-large",
                429 => "rate-limited",
                400..=499 => "invalid",
                _ => "provider",
            };
            let message = match code {
                "unauthorized" => format!("The provider refused the API key ({detail})."),
                "rate-limited" => format!("The provider is rate limiting requests ({detail})."),
                "invalid" if status == 404 => {
                    format!("The base URL or model was not found ({detail}).")
                }
                _ => format!("The provider returned an error ({detail})."),
            };
            AiError::new(code, message)
        }
        ureq::Error::Transport(t) => {
            let text = t.to_string();
            if text.contains("timed out") || text.contains("Timeout") {
                AiError::new("timeout", "The provider did not answer in time.")
            } else {
                AiError::new("offline", "Could not reach the provider.")
            }
        }
    }
}

fn read_reply(response: ureq::Response) -> Result<Value, AiError> {
    parse_reply(response.into_reader())
}

fn parse_reply(reader: impl Read) -> Result<Value, AiError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_RESPONSE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| AiError::new("provider", "Could not read the provider reply."))?;
    if bytes.len() > MAX_RESPONSE {
        return Err(AiError::new(
            "too-large",
            "The provider reply exceeds 1 MiB.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AiError::new("provider", "The provider reply was not valid JSON."))
}

/// Runs one completion.
pub fn complete(
    config: &AiConfig,
    key: Option<&str>,
    prompt: &Prompt,
) -> Result<Completion, AiError> {
    if config.base_url.is_empty() {
        return Err(AiError::new(
            "not-configured",
            "Set the provider's base URL in Settings → AI.",
        ));
    }
    if prompt.model.is_empty() {
        return Err(AiError::new(
            "not-configured",
            "Choose a model in Settings → AI.",
        ));
    }
    let mut messages = Vec::new();
    if !prompt.system.is_empty() {
        messages.push(json!({ "role": "system", "content": prompt.system }));
    }
    messages.push(json!({ "role": "user", "content": prompt.prompt }));
    let body = json!({
        "model": prompt.model,
        "messages": messages,
        "max_tokens": prompt.max_tokens,
        "temperature": prompt.temperature,
    });
    let reply = read_reply(
        request(config, key, "POST", "chat/completions")
            .send_json(body)
            .map_err(map_error)?,
    )?;
    let text = reply["choices"][0]["message"]["content"]
        .as_str()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if text.is_empty() {
        return Err(AiError::new("provider", "The provider returned no text."));
    }
    Ok(Completion {
        text,
        model: reply["model"].as_str().unwrap_or(&prompt.model).to_string(),
        input_tokens: reply["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: reply["usage"]["completion_tokens"].as_u64().unwrap_or(0),
    })
}

/// Model ids the provider lists, if it supports `GET /models`.
pub fn models(config: &AiConfig, key: Option<&str>) -> Result<Vec<String>, AiError> {
    if config.base_url.is_empty() {
        return Err(AiError::new(
            "not-configured",
            "Set the provider's base URL first.",
        ));
    }
    let reply = read_reply(
        request(config, key, "GET", "models")
            .call()
            .map_err(map_error)?,
    )?;
    let mut ids: Vec<String> = reply["data"]
        .as_array()
        .or(reply["models"].as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m["id"].as_str().or(m["name"].as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod security_tests {
    use super::*;
    #[test]
    fn response_limit_applies_before_parsing_and_errors_do_not_echo_data() {
        let oversized = vec![b' '; MAX_RESPONSE + 1];
        assert_eq!(
            parse_reply(oversized.as_slice()).unwrap_err().code,
            "too-large"
        );
        let invalid = parse_reply(b"PRIVATE_TRANSCRIPT".as_slice()).unwrap_err();
        assert_eq!(invalid.message, "The provider reply was not valid JSON.");
        assert_eq!(
            parse_reply(b"{\"data\":[]}".as_slice()).unwrap(),
            json!({"data":[]})
        );
    }
}
