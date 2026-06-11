use anyhow::{bail, Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{read, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug)]
pub enum Provider {
    OpenAI,
    Anthropic,
    Google,
    OpenAICompat,
    LMStudio,
}

#[derive(Clone)]
pub struct Msg {
    pub role: &'static str,
    pub text: String,
}

impl Msg {
    pub fn new(role: &'static str, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
        }
    }
}

pub struct Config {
    pub provider: Provider,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub base_system: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct ConfigProfile {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_system_prompt: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Shortcut {
    pub key: String,
    pub command: String,
}

pub(crate) fn config_dir_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("biscuits"))
}

fn legacy_config_dir_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("biscuit-code"))
}

fn config_file_path() -> Option<PathBuf> {
    config_dir_path().map(|d| d.join("config.json"))
}

fn legacy_config_file_path() -> Option<PathBuf> {
    legacy_config_dir_path().map(|d| d.join("config.json"))
}

pub fn load_profile() -> Option<ConfigProfile> {
    let path = config_file_path()?;
    if let Ok(text) = fs::read_to_string(&path) {
        return serde_json::from_str(&text).ok();
    }
    let legacy = legacy_config_file_path()?;
    let text = fs::read_to_string(legacy).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_profile(profile: &ConfigProfile) -> Result<()> {
    let dir = config_dir_path().context("cannot determine config directory")?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let json = serde_json::to_string_pretty(profile)?;
    fs::write(&path, format!("{json}\n"))?;
    Ok(())
}

pub fn clear_profile() -> Result<String> {
    let mut removed = false;
    if let Some(path) = config_file_path() {
        if path.exists() {
            fs::remove_file(&path)?;
            removed = true;
        }
    }
    if let Some(path) = legacy_config_file_path() {
        if path.exists() {
            fs::remove_file(&path)?;
            removed = true;
        }
    }
    if removed {
        Ok("config profile cleared".into())
    } else {
        Ok("no saved config profile found".into())
    }
}

pub fn show_profile() -> String {
    match load_profile() {
        Some(p) => {
            let mut out = format!(
                "saved config profile:\n  provider: {}\n  model: {}\n  base_url: {}",
                p.provider, p.model, p.base_url
            );
            if let Some(ref prompt) = p.custom_system_prompt {
                out.push_str(&format!(
                    "\n  custom_prompt: {}",
                    truncate_display(prompt, 120)
                ));
            }
            out.push_str(&format!(
                "\n  path: {}",
                config_file_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
            out
        }
        None => "no saved config profile".into(),
    }
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max).collect::<String>())
    }
}

pub fn load_shortcuts() -> Vec<Shortcut> {
    let Some(dir) = config_dir_path() else {
        return Vec::new();
    };
    let path = dir.join("shortcuts.json");
    let text = fs::read_to_string(path).ok().or_else(|| {
        legacy_config_dir_path().and_then(|dir| fs::read_to_string(dir.join("shortcuts.json")).ok())
    });
    let Some(text) = text else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_shortcuts(shortcuts: &[Shortcut]) -> Result<()> {
    let dir = config_dir_path().context("cannot determine config directory")?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("shortcuts.json");
    let json = serde_json::to_string_pretty(shortcuts)?;
    fs::write(&path, format!("{json}\n"))?;
    Ok(())
}

pub fn show_shortcuts() -> String {
    let shortcuts = load_shortcuts();
    if shortcuts.is_empty() {
        return "no shortcuts configured. use /shortcut add <key> <command>".into();
    }
    let mut out = String::from("shortcuts:\n");
    for s in &shortcuts {
        out.push_str(&format!("  {} → {}\n", s.key, s.command));
    }
    out.trim_end().to_string()
}

pub fn expand_shortcut(input: &str) -> Option<String> {
    let shortcuts = load_shortcuts();
    let input_lower = input.trim().to_lowercase();
    shortcuts
        .iter()
        .find(|s| s.key.to_lowercase() == input_lower)
        .map(|s| s.command.clone())
}

#[derive(Default)]
pub struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    total: Option<u64>,
    estimated: bool,
}

#[derive(Clone, Copy, Default, serde::Deserialize, serde::Serialize)]
pub struct UsageSnapshot {
    pub input: u64,
    pub output: u64,
    pub total: u64,
    pub estimated: bool,
}

impl Usage {
    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            input: self.input.unwrap_or(0),
            output: self.output.unwrap_or(0),
            total: self.total.unwrap_or(0),
            estimated: self.estimated,
        }
    }
}

#[derive(Default)]
pub struct Totals {
    turns: u64,
    tokens: u64,
    estimated: bool,
}

pub async fn setup(client: &Client) -> Result<Config> {
    let cwd = env::current_dir()?.display().to_string();
    let default_prompt = format!(
        "You are Biscuits, a fast and practical AI agent launched in {cwd}. Treat this directory as the workspace. Use the available tools when they help, and be direct about what you did."
    );

    // ── Try saved config profile ──
    if let Some(profile) = load_profile() {
        println!("\nsaved config profile found:");
        println!("  provider: {}", profile.provider);
        println!("  model: {}", profile.model);
        println!("  base_url: {}", profile.base_url);
        if profile.custom_system_prompt.is_some() {
            println!("  custom_prompt: (set)");
        }
        let use_saved = ask("Use saved config?", "Y")?;
        if use_saved.eq_ignore_ascii_case("y")
            || use_saved.eq_ignore_ascii_case("yes")
            || use_saved.is_empty()
        {
            let provider = match profile.provider.to_lowercase().as_str() {
                "openai" => Provider::OpenAI,
                "anthropic" => Provider::Anthropic,
                "google" => Provider::Google,
                "openai_compatible" | "openai_compat" => Provider::OpenAICompat,
                "lm_studio" | "lmstudio" => Provider::LMStudio,
                _ => bail!("unknown provider in saved profile: {}", profile.provider),
            };
            let env_key = match provider {
                Provider::OpenAI => "OPENAI_API_KEY",
                Provider::Anthropic => "ANTHROPIC_API_KEY",
                Provider::Google => "GEMINI_API_KEY",
                Provider::OpenAICompat => "OPENAI_COMPAT_API_KEY",
                Provider::LMStudio => "LMSTUDIO_API_KEY",
            };
            let default_key = match provider {
                Provider::OpenAICompat => Some(""),
                Provider::LMStudio => Some("lm-studio"),
                _ => None,
            };
            let api_key = ask_key("API key", env_key, default_key)?;
            let base_system = profile
                .custom_system_prompt
                .clone()
                .unwrap_or(default_prompt.clone());
            let mut config = Config {
                provider,
                api_key,
                base_url: profile.base_url,
                model: profile.model,
                base_system,
            };
            if matches!(config.provider, Provider::LMStudio) && config.model.is_empty() {
                config.model = first_openai_model(client, &config)
                    .await
                    .context("LM Studio is running, but I could not read /v1/models")?;
                println!("model: {}", config.model);
            }
            if config.model.is_empty() {
                bail!("model is required");
            }
            return Ok(config);
        }
    }

    // ── Normal interactive setup ──
    let provider = pick_provider()?;

    let mut config = match provider {
        Provider::OpenAI => Config {
            provider,
            api_key: ask_key("OpenAI API key", "OPENAI_API_KEY", None)?,
            base_url: "https://api.openai.com/v1".into(),
            model: ask("Model", "gpt-4o-mini")?,
            base_system: default_prompt.clone(),
        },
        Provider::Anthropic => Config {
            provider,
            api_key: ask_key("Anthropic API key", "ANTHROPIC_API_KEY", None)?,
            base_url: "https://api.anthropic.com/v1".into(),
            model: ask("Model", "claude-sonnet-4-20250514")?,
            base_system: default_prompt.clone(),
        },
        Provider::Google => Config {
            provider,
            api_key: ask_key("Google API key", "GEMINI_API_KEY", None)?,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            model: ask("Model", "gemini-2.0-flash")?,
            base_system: default_prompt.clone(),
        },
        Provider::OpenAICompat => Config {
            provider,
            api_key: ask_key("API key", "OPENAI_COMPAT_API_KEY", Some(""))?,
            base_url: ask("Base URL", "http://localhost:8000/v1")?,
            model: ask("Model", "")?,
            base_system: default_prompt.clone(),
        },
        Provider::LMStudio => Config {
            provider,
            api_key: ask_key("LM Studio API key", "LMSTUDIO_API_KEY", Some("lm-studio"))?,
            base_url: ask("Base URL", "http://localhost:1234/v1")?,
            model: ask("Model (blank = first loaded model)", "")?,
            base_system: default_prompt.clone(),
        },
    };

    if matches!(config.provider, Provider::LMStudio) && config.model.is_empty() {
        config.model = first_openai_model(client, &config)
            .await
            .context("LM Studio is running, but I could not read /v1/models")?;
        println!("model: {}", config.model);
    }
    if config.model.is_empty() {
        bail!("model is required");
    }

    // ── Save profile (no API key) ──
    let profile = ConfigProfile {
        provider: provider_name(config.provider).to_string(),
        model: config.model.clone(),
        base_url: config.base_url.clone(),
        custom_system_prompt: None,
    };
    if let Err(err) = save_profile(&profile) {
        eprintln!("warning: could not save config profile: {err}");
    }

    Ok(config)
}

pub async fn from_env(client: &Client) -> Result<Config> {
    let provider = match env::var("AI_PROVIDER")
        .unwrap_or_else(|_| "openai".into())
        .to_lowercase()
        .as_str()
    {
        "openai" => Provider::OpenAI,
        "anthropic" | "claude" => Provider::Anthropic,
        "google" | "gemini" => Provider::Google,
        "openai_compat" | "openai-compatible" | "compat" => Provider::OpenAICompat,
        "lmstudio" | "lm_studio" | "lm-studio" => Provider::LMStudio,
        other => bail!("unknown AI_PROVIDER: {other}"),
    };

    let cwd = env::current_dir()?.display().to_string();
    let base_system = load_profile()
        .and_then(|p| p.custom_system_prompt)
        .unwrap_or_else(|| format!(
            "You are Biscuits, a fast and practical AI agent launched in {cwd}. Treat this directory as the workspace. Use the available tools when they help, and be direct about what you did."
        ));

    let mut config = match provider {
        Provider::OpenAI => Config {
            provider,
            api_key: env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is required")?,
            base_url: env::var("AI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            model: env::var("AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            base_system,
        },
        Provider::Anthropic => Config {
            provider,
            api_key: env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is required")?,
            base_url: env::var("AI_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com/v1".into()),
            model: env::var("AI_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into()),
            base_system,
        },
        Provider::Google => Config {
            provider,
            api_key: env::var("GEMINI_API_KEY").context("GEMINI_API_KEY is required")?,
            base_url: env::var("AI_BASE_URL")
                .unwrap_or_else(|_| "https://generativelanguage.googleapis.com/v1beta".into()),
            model: env::var("AI_MODEL").unwrap_or_else(|_| "gemini-2.0-flash".into()),
            base_system,
        },
        Provider::OpenAICompat => Config {
            provider,
            api_key: env::var("OPENAI_COMPAT_API_KEY").unwrap_or_default(),
            base_url: env::var("AI_BASE_URL").unwrap_or_else(|_| "http://localhost:8000/v1".into()),
            model: env::var("AI_MODEL")
                .context("AI_MODEL is required for OpenAI-compatible providers")?,
            base_system,
        },
        Provider::LMStudio => Config {
            provider,
            api_key: env::var("LMSTUDIO_API_KEY").unwrap_or_else(|_| "lm-studio".into()),
            base_url: env::var("AI_BASE_URL").unwrap_or_else(|_| "http://localhost:1234/v1".into()),
            model: env::var("AI_MODEL").unwrap_or_default(),
            base_system,
        },
    };

    if matches!(config.provider, Provider::LMStudio) && config.model.is_empty() {
        config.model = first_openai_model(client, &config).await?;
    }
    if config.model.is_empty() {
        bail!("AI_MODEL is required");
    }
    Ok(config)
}

pub fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenAI => "openai",
        Provider::Anthropic => "anthropic",
        Provider::Google => "google",
        Provider::OpenAICompat => "openai_compatible",
        Provider::LMStudio => "lm_studio",
    }
}

pub async fn chat(
    client: &Client,
    config: &Config,
    history: &[Msg],
    system_context: &str,
    input_chars: usize,
) -> Result<(String, Usage)> {
    chat_inner(client, config, history, system_context, input_chars, true).await
}

pub async fn chat_capture(
    client: &Client,
    config: &Config,
    history: &[Msg],
    system_context: &str,
    input_chars: usize,
) -> Result<(String, Usage)> {
    chat_inner(client, config, history, system_context, input_chars, false).await
}

async fn chat_inner(
    client: &Client,
    config: &Config,
    history: &[Msg],
    system_context: &str,
    input_chars: usize,
    print_tokens: bool,
) -> Result<(String, Usage)> {
    let system_context = format!("{}\n\n{}", config.base_system, system_context);
    let req = match config.provider {
        Provider::OpenAI | Provider::OpenAICompat | Provider::LMStudio => {
            openai_request(client, config, history, &system_context, true)
        }
        Provider::Anthropic => anthropic_request(client, config, history, &system_context, true),
        Provider::Google => google_request(client, config, history, &system_context, true),
    };

    let response = send(req).await?;
    stream_response(config.provider, response, input_chars, print_tokens).await
}

pub async fn complete(
    client: &Client,
    config: &Config,
    system: &str,
    prompt: &str,
) -> Result<String> {
    let history = [Msg::new("user", prompt)];
    complete_history(client, config, system, &history).await
}

pub async fn complete_history(
    client: &Client,
    config: &Config,
    system: &str,
    history: &[Msg],
) -> Result<String> {
    let system = format!("{}\n\n{}", config.base_system, system);
    let req = match config.provider {
        Provider::OpenAI | Provider::OpenAICompat | Provider::LMStudio => {
            openai_request(client, config, history, &system, false)
        }
        Provider::Anthropic => anthropic_request(client, config, history, &system, false),
        Provider::Google => google_request(client, config, history, &system, false),
    };

    let event: Value = send(req).await?.json().await?;
    let text = match config.provider {
        Provider::OpenAI | Provider::OpenAICompat | Provider::LMStudio => {
            event.pointer("/choices/0/message/content")
        }
        Provider::Anthropic => event.pointer("/content/0/text"),
        Provider::Google => event.pointer("/candidates/0/content/parts/0/text"),
    }
    .and_then(Value::as_str)
    .unwrap_or_default()
    .to_string();

    Ok(text)
}

fn pick_provider() -> Result<Provider> {
    let providers = [
        ("OpenAI", Provider::OpenAI),
        ("Anthropic", Provider::Anthropic),
        ("Google", Provider::Google),
        ("OpenAI compatible", Provider::OpenAICompat),
        ("LM Studio", Provider::LMStudio),
    ];
    let mut selected = 0usize;
    let mut out = io::stdout();

    let mut raw_mode = RawModeGuard::enable()?;
    loop {
        execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
        println!("Choose provider with Up/Down, then Enter\n");
        for (i, (name, _)) in providers.iter().enumerate() {
            println!("{} {}", if i == selected { ">" } else { " " }, name);
        }
        out.flush()?;

        if let Event::Key(key) = read()? {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(providers.len() - 1),
                KeyCode::Enter => {
                    raw_mode.disable()?;
                    println!();
                    return Ok(providers[selected].1);
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    bail!("cancelled");
                }
                _ => {}
            }
        }
    }
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enable() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self { enabled: true })
    }

    fn disable(&mut self) -> Result<()> {
        if self.enabled {
            disable_raw_mode()?;
            self.enabled = false;
        }
        Ok(())
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
        }
    }
}

fn ask(label: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        print!("{label}: ");
    } else {
        print!("{label} [{default}]: ");
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    Ok(if input.is_empty() {
        default.to_string()
    } else {
        input.to_string()
    })
}

fn ask_key(label: &str, env_name: &str, default: Option<&str>) -> Result<String> {
    let env_value = env::var(env_name).ok();
    let hint = if env_value.is_some() {
        format!(" [{env_name}]")
    } else if let Some(default) = default {
        if default.is_empty() {
            String::new()
        } else {
            format!(" [{default}]")
        }
    } else {
        String::new()
    };

    let input = rpassword::prompt_password(format!("{label}{hint}: "))?;
    let key = if input.trim().is_empty() {
        env_value
            .or_else(|| default.map(str::to_string))
            .unwrap_or_default()
    } else {
        input.trim().to_string()
    };

    if key.is_empty() && default.is_none() {
        bail!("{label} is required");
    }
    Ok(key)
}

async fn first_openai_model(client: &Client, config: &Config) -> Result<String> {
    let mut req = client.get(format!("{}/models", clean_base(&config.base_url)));
    if !config.api_key.is_empty() {
        req = req.bearer_auth(&config.api_key);
    }
    let json: Value = send(req).await?.json().await?;
    json["data"][0]["id"]
        .as_str()
        .map(str::to_string)
        .context("no loaded models found; load a model in LM Studio first")
}

/// Collapse consecutive same-role messages into one. A single assistant tool
/// plan can produce several `user` tool-result messages in a row; Anthropic and
/// Gemini reject non-alternating roles with HTTP 400, so every provider gets a
/// merged, strictly-alternating history and behaves identically.
fn merge_adjacent(history: &[Msg]) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for m in history {
        if let Some(last) = out.last_mut() {
            if last.0 == m.role {
                last.1.push_str("\n\n");
                last.1.push_str(&m.text);
                continue;
            }
        }
        out.push((m.role, m.text.clone()));
    }
    out
}

fn openai_request(
    client: &Client,
    config: &Config,
    history: &[Msg],
    system_context: &str,
    stream: bool,
) -> RequestBuilder {
    let mut messages = vec![json!({ "role": "system", "content": system_context })];
    messages.extend(
        merge_adjacent(history)
            .into_iter()
            .map(|(role, text)| json!({ "role": role, "content": text })),
    );

    let mut body = json!({
        "model": config.model,
        "messages": messages,
        "stream": stream
    });
    if stream && matches!(config.provider, Provider::OpenAI) {
        body["stream_options"] = json!({ "include_usage": true });
    }

    authed(
        client
            .post(format!("{}/chat/completions", clean_base(&config.base_url)))
            .json(&body),
        &config.api_key,
    )
}

fn anthropic_request(
    client: &Client,
    config: &Config,
    history: &[Msg],
    system_context: &str,
    stream: bool,
) -> RequestBuilder {
    let messages: Vec<_> = merge_adjacent(history)
        .into_iter()
        .map(|(role, text)| json!({ "role": role, "content": text }))
        .collect();

    client
        .post(format!("{}/messages", clean_base(&config.base_url)))
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": config.model,
            "system": system_context,
            "messages": messages,
            "max_tokens": 4096,
            "stream": stream
        }))
}

fn google_request(
    client: &Client,
    config: &Config,
    history: &[Msg],
    system_context: &str,
    stream: bool,
) -> RequestBuilder {
    let contents: Vec<_> = merge_adjacent(history)
        .into_iter()
        .map(|(role, text)| {
            let role = if role == "assistant" { "model" } else { "user" };
            json!({ "role": role, "parts": [{ "text": text }] })
        })
        .collect();
    let model = config
        .model
        .strip_prefix("models/")
        .unwrap_or(&config.model);
    let method = if stream {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    };

    // Send the key in the header, not the query string: a URL with the key in it
    // leaks into transport-error messages and any place the request URL is logged.
    client
        .post(format!(
            "{}/models/{}:{}",
            clean_base(&config.base_url),
            model,
            method
        ))
        .header("x-goog-api-key", &config.api_key)
        .json(&json!({
            "systemInstruction": { "parts": [{ "text": system_context }] },
            "contents": contents
        }))
}

fn authed(req: RequestBuilder, key: &str) -> RequestBuilder {
    if key.is_empty() {
        req
    } else {
        req.bearer_auth(key)
    }
}

async fn send(req: RequestBuilder) -> Result<reqwest::Response> {
    let response = req.send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("request failed ({status}): {body}");
    }
    Ok(response)
}

async fn stream_response(
    provider: Provider,
    response: reqwest::Response,
    input_chars: usize,
    print_tokens: bool,
) -> Result<(String, Usage)> {
    let mut chunks = response.bytes_stream();
    // Accumulate RAW bytes and only decode complete lines. Decoding each network
    // chunk individually (the old behavior) mangled any multi-byte UTF-8
    // character whose bytes straddled two chunks into replacement characters.
    // A newline byte (0x0A) can never appear inside a multi-byte sequence, so
    // splitting on it first and decoding whole lines is always safe.
    let mut buf: Vec<u8> = Vec::new();
    let mut answer = String::new();
    let mut usage = Usage::default();

    while let Some(chunk) = chunks.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            let decoded = String::from_utf8_lossy(&line_bytes);
            let line = decoded.trim_end_matches('\n').trim_end_matches('\r');
            if handle_sse_line(provider, line, &mut answer, &mut usage, print_tokens)? {
                finish_usage(&mut usage, input_chars, answer.len());
                return Ok((answer, usage));
            }
        }
    }

    if !buf.is_empty() {
        let decoded = String::from_utf8_lossy(&buf);
        let line = decoded.trim();
        if !line.is_empty() {
            handle_sse_line(provider, line, &mut answer, &mut usage, print_tokens)?;
        }
    }
    finish_usage(&mut usage, input_chars, answer.len());
    Ok((answer, usage))
}

fn handle_sse_line(
    provider: Provider,
    line: &str,
    answer: &mut String,
    usage: &mut Usage,
    print_tokens: bool,
) -> Result<bool> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(true);
    }

    let event: Value = serde_json::from_str(data)?;
    if let Some(message) = event.pointer("/error/message").and_then(Value::as_str) {
        bail!("{message}");
    }

    match provider {
        Provider::OpenAI | Provider::OpenAICompat | Provider::LMStudio => {
            emit(
                event.pointer("/choices/0/delta/content"),
                answer,
                print_tokens,
            )?;
            read_usage(
                event.get("usage"),
                usage,
                "prompt_tokens",
                "completion_tokens",
            );
        }
        Provider::Anthropic => match event["type"].as_str().unwrap_or_default() {
            "content_block_delta" => emit(event.pointer("/delta/text"), answer, print_tokens)?,
            "message_start" => read_usage(
                event.pointer("/message/usage"),
                usage,
                "input_tokens",
                "output_tokens",
            ),
            "message_delta" => {
                read_usage(event.get("usage"), usage, "input_tokens", "output_tokens")
            }
            "message_stop" => return Ok(true),
            _ => {}
        },
        Provider::Google => {
            emit(
                event.pointer("/candidates/0/content/parts/0/text"),
                answer,
                print_tokens,
            )?;
            read_usage(
                event.get("usageMetadata"),
                usage,
                "promptTokenCount",
                "candidatesTokenCount",
            );
        }
    }

    Ok(false)
}

fn emit(value: Option<&Value>, answer: &mut String, print_tokens: bool) -> Result<()> {
    if let Some(text) = value.and_then(Value::as_str) {
        if print_tokens {
            print!("{text}");
            io::stdout().flush()?;
        }
        answer.push_str(text);
    }
    Ok(())
}

fn read_usage(value: Option<&Value>, usage: &mut Usage, input: &str, output: &str) {
    if let Some(v) = value {
        usage.input = v.get(input).and_then(Value::as_u64).or(usage.input);
        usage.output = v.get(output).and_then(Value::as_u64).or(usage.output);
        usage.total = v
            .get("total_tokens")
            .or_else(|| v.get("totalTokenCount"))
            .and_then(Value::as_u64)
            .or(usage.total);
    }
}

fn finish_usage(usage: &mut Usage, input_chars: usize, output_chars: usize) {
    if usage.input.is_none() || usage.output.is_none() {
        usage.estimated = true;
        usage.input.get_or_insert(estimate_tokens(input_chars));
        usage.output.get_or_insert(estimate_tokens(output_chars));
    }
    usage
        .total
        .get_or_insert(usage.input.unwrap_or(0) + usage.output.unwrap_or(0));
}

fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

pub fn print_usage_snapshot(usage: UsageSnapshot, totals: &mut Totals) {
    totals.turns += 1;
    totals.tokens += usage.total;
    totals.estimated |= usage.estimated;

    let mark = if usage.estimated { " approx" } else { "" };
    let total_mark = if totals.estimated { " approx" } else { "" };
    println!(
        "\n[tokens{mark}] input={} output={} total={} | session turns={} total={}{}",
        usage.input, usage.output, usage.total, totals.turns, totals.tokens, total_mark
    );
}

fn clean_base(url: &str) -> &str {
    url.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_consecutive_same_role_messages() {
        let history = vec![
            Msg::new("user", "question"),
            Msg::new("assistant", "plan"),
            Msg::new("user", "result one"),
            Msg::new("user", "result two"),
        ];
        let merged = merge_adjacent(&history);
        // The two consecutive user messages collapse into one, so the history
        // stays strictly alternating (what Anthropic/Gemini require).
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].0, "user");
        assert_eq!(merged[1].0, "assistant");
        assert_eq!(merged[2].0, "user");
        assert_eq!(merged[2].1, "result one\n\nresult two");
    }

    #[test]
    fn merge_is_identity_when_already_alternating() {
        let history = vec![
            Msg::new("user", "a"),
            Msg::new("assistant", "b"),
            Msg::new("user", "c"),
        ];
        let merged = merge_adjacent(&history);
        assert_eq!(merged.len(), 3);
    }
}
