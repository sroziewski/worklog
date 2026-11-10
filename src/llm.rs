//! Ticket drafting with the LLM opencode is configured for (an OpenAI-compatible endpoint, the
//! provider named by `tickets.llm_provider`), reusing opencode's endpoint and key.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config::TicketConfig;

pub const MAX_DRAFTS: usize = 3;
/// Jira rejects longer summaries.
pub const MAX_TITLE_CHARS: usize = 255;

#[derive(Clone)]
pub struct LlmCredentials {
    /// Base URL; `/chat/completions` is appended.
    pub url: String,
    pub api_key: String,
    pub model: String,
}

impl std::fmt::Debug for LlmCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmCredentials").field("url", &self.url).field("api_key", &"<redacted>").field("model", &self.model).finish()
    }
}

/// opencode writes `{env:NAME}` for values taken from the environment.
fn expand(v: &str, env: &impl Fn(&str) -> Option<String>) -> Option<String> {
    match v.strip_prefix("{env:").and_then(|r| r.strip_suffix('}')) {
        Some(name) => env(name),
        None => Some(v.to_owned()),
    }
}

/// Endpoint, key and model: `LLM_URL` / `LLM_API_KEY` / `LLM_MODEL` from the environment first,
/// then `llm_url` / `llm_model` from the config, then the provider entry in opencode's config.
/// opencode's file is read only when the first two leave the endpoint or key unset, so a file it
/// cannot parse (opencode itself accepts comments) never blocks explicit settings.
pub fn credentials(cfg: &TicketConfig, env: impl Fn(&str) -> Option<String>) -> Result<LlmCredentials> {
    let nonempty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let mut url = nonempty(env("LLM_URL")).or_else(|| nonempty(cfg.llm_url.clone()));
    let mut api_key = nonempty(env("LLM_API_KEY"));

    if (url.is_none() || api_key.is_none())
        && let Some(p) = cfg.opencode_config.as_ref().filter(|p| p.exists())
    {
        let v = read_opencode(p)?;
        // The user's opencode writes `providers.<name>.settings`; upstream's schema is
        // `provider.<name>.options`.
        let provider = v
            .pointer(&format!("/providers/{}/settings", cfg.llm_provider))
            .or_else(|| v.pointer(&format!("/provider/{}/options", cfg.llm_provider)));
        let from_provider = |k: &str| provider.and_then(|s| s[k].as_str()).and_then(|v| expand(v, &env));
        url = url.or_else(|| nonempty(from_provider("baseURL")));
        api_key = api_key.or_else(|| nonempty(from_provider("apiKey")));
    }

    // No built-in provider: an unset one must say where to name it, not look up "provider ''".
    let provider = if cfg.llm_provider.trim().is_empty() {
        "a provider (WORKLOG_LLM_PROVIDER in .env or tickets.llm_provider)".to_owned()
    } else {
        format!("provider '{}'", cfg.llm_provider)
    };
    let url = url.with_context(|| format!("no LLM endpoint: set LLM_URL, tickets.llm_url, or {provider} in opencode's config"))?;
    let api_key = api_key.with_context(|| format!("no LLM API key: set LLM_API_KEY or {provider} apiKey in opencode's config"))?;
    let model = nonempty(env("LLM_MODEL"))
        .or_else(|| nonempty(Some(cfg.llm_model.clone())))
        .context("no LLM model: set LLM_MODEL, WORKLOG_LLM_MODEL in .env, or tickets.llm_model")?;
    Ok(LlmCredentials { url: url.trim_end_matches('/').to_owned(), api_key, model })
}

fn read_opencode(p: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", p.display()))
}

pub struct Llm {
    creds: LlmCredentials,
    agent: ureq::Agent,
}

impl Llm {
    pub fn new(creds: LlmCredentials) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { creds, agent }
    }

    /// One non-streaming chat completion; returns the assistant message text.
    pub fn chat(&self, system: &str, user: &str) -> Result<String> {
        let url = format!("{}/chat/completions", self.creds.url);
        let body = json!({
            "model": self.creds.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "temperature": 0.2,
            "stream": false,
        });
        let t = std::time::Instant::now();
        let mut resp = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.creds.api_key))
            .header("Accept", "application/json")
            .send_json(&body)
            .map_err(|e| anyhow!("LLM request to {url}: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        log::info!("LLM {} -> {status} in {:.2}s", self.creds.model, t.elapsed().as_secs_f64());
        match status {
            200..=299 => {}
            401 | 403 => bail!("LLM refused the API key ({status}); check opencode's provider key or LLM_API_KEY"),
            _ => bail!("LLM {status}: {}", text.chars().take(300).collect::<String>()),
        }
        let v: Value = serde_json::from_str(&text).context("LLM response is not JSON")?;
        v.pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty())
            .context("LLM response has no message content")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TicketDraft {
    pub title: String,
    pub description: String,
    /// The work items the ticket covers, as 0-based indices into the list sent to the model;
    /// empty when the model named none. Indices past that list are not filtered out here.
    pub items: Vec<usize>,
}

/// The JSON in a model answer that should be JSON but may come after the model's reasoning,
/// in a code fence, or among prose.
pub fn json_answer(answer: &str) -> Result<Value> {
    // Reasoning models may prepend their thinking, which can contain braces of its own.
    let answer = match answer.rfind("</think>") {
        Some(i) => &answer[i + "</think>".len()..],
        None => answer,
    };
    extract_json(answer).with_context(|| format!("LLM answer holds no JSON: {}", answer.chars().take(200).collect::<String>()))
}

fn extract_json(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(s.trim()) {
        return Some(v);
    }
    // Fenced or surrounded by prose: try each opening bracket against the last matching closer,
    // the one that comes first in the text first, so `[{...}]` stays the array, not its element.
    let mut pairs = [('{', '}'), ('[', ']')];
    pairs.sort_by_key(|(open, _)| s.find(*open).unwrap_or(usize::MAX));
    for (open, close) in pairs {
        let (Some(a), Some(b)) = (s.find(open), s.rfind(close)) else { continue };
        if a < b
            && let Ok(v) = serde_json::from_str::<Value>(&s[a..=b])
        {
            return Some(v);
        }
    }
    None
}
