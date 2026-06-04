// Model-provider routing. noted is local-first ($0, private) by default; the
// "Balanced" mode offloads only the latency-sensitive *structured* calls
// (note extraction + photo OCR) to Gemini so a busy/wedged local Ollama never
// leaves a note stuck on "reading". Embeddings and the conversational assistant
// deliberately stay local (cheap + most personal).
//
// Design notes:
// - Config (mode + model ids) lives in a small JSON file in the app data dir.
// - The API key lives in the macOS Keychain (never on disk, never in the JSON),
//   accessed through the `security` CLI so we add no new crate / Cargo.lock churn.
// - A process-global holds the live config so `ollama::chat_json` can consult it
//   without threading app state through every call site (keeps pipeline.rs untouched).

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{OnceLock, RwLock};

const KEYCHAIN_SERVICE: &str = "com.noted.app";
const KEYCHAIN_ACCOUNT: &str = "gemini_api_key";
// Gemini speaks an OpenAI-compatible dialect at this base (chat/completions).
const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const CONFIG_FILE: &str = "provider.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 100% local Ollama. The default. $0, fully private.
    Local,
    /// Local embeddings + chat, but extract/OCR go to Gemini (fast + cheap).
    Balanced,
}

impl Mode {
    fn parse(s: &str) -> Mode {
        match s.to_ascii_lowercase().as_str() {
            "balanced" => Mode::Balanced,
            _ => Mode::Local,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub mode: Mode,
    pub gemini_text_model: String,   // extraction hot path — cheapest/fastest
    pub gemini_vision_model: String, // handwriting OCR — a touch stronger
    // Loaded from the Keychain at startup; never serialized to the JSON file.
    #[serde(skip)]
    pub gemini_api_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: Mode::Local,
            gemini_text_model: "gemini-2.5-flash-lite".into(),
            gemini_vision_model: "gemini-2.5-flash".into(),
            gemini_api_key: None,
        }
    }
}

static CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();
fn cell() -> &'static RwLock<Config> {
    CONFIG.get_or_init(|| RwLock::new(Config::default()))
}

/// Snapshot of the current config (cheap clone).
pub fn get() -> Config {
    cell().read().unwrap().clone()
}

/// True when structured calls (extract + OCR) should be routed to the cloud.
pub fn use_cloud_for_extract() -> bool {
    let c = get();
    c.mode == Mode::Balanced && c.gemini_api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false)
}

// ── Keychain (via the macOS `security` CLI) ─────────────────────────────────
fn keychain_read() -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if key.is_empty() { None } else { Some(key) }
}

fn keychain_write(key: &str) -> Result<()> {
    // -U updates the item if it already exists.
    let status = Command::new("security")
        .args(["add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w", key])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("failed to write key to keychain"))
    }
}

fn keychain_delete() {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT])
        .status();
}

// ── Config file (mode + model ids only — never the key) ─────────────────────
fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

fn write_config_file(dir: &Path, c: &Config) {
    if let Ok(s) = serde_json::to_string_pretty(c) {
        let _ = std::fs::write(config_path(dir), s);
    }
}

/// Load config from disk + key from the Keychain into the process global.
/// Called once at startup.
pub fn init(dir: &Path) {
    let mut c = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default();
    c.gemini_api_key = keychain_read();
    *cell().write().unwrap() = c;
}

/// Apply settings changes from the UI: update mode/models, store-or-clear the
/// key in the Keychain, persist the JSON, and refresh the global.
/// `gemini_api_key`: Some(non-empty) sets it, Some("") clears it, None leaves it.
pub fn update(
    dir: &Path,
    mode: &str,
    gemini_api_key: Option<String>,
    gemini_text_model: Option<String>,
    gemini_vision_model: Option<String>,
) -> Result<()> {
    let mut c = get();
    c.mode = Mode::parse(mode);
    if let Some(m) = gemini_text_model {
        if !m.trim().is_empty() {
            c.gemini_text_model = m.trim().to_string();
        }
    }
    if let Some(m) = gemini_vision_model {
        if !m.trim().is_empty() {
            c.gemini_vision_model = m.trim().to_string();
        }
    }
    match gemini_api_key {
        Some(k) if !k.trim().is_empty() => {
            keychain_write(k.trim())?;
            c.gemini_api_key = Some(k.trim().to_string());
        }
        Some(_) => {
            keychain_delete();
            c.gemini_api_key = None;
        }
        None => {} // unchanged
    }
    write_config_file(dir, &c);
    *cell().write().unwrap() = c;
    Ok(())
}

// ── Gemini (OpenAI-compatible chat/completions) ─────────────────────────────
fn data_url(b64: &str) -> String {
    // Cheap magic-byte sniff so the mime is right for Gemini's vision input.
    let mime = if b64.starts_with("iVBOR") {
        "image/png"
    } else if b64.starts_with("R0lGOD") {
        "image/gif"
    } else if b64.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/jpeg"
    };
    format!("data:{mime};base64,{b64}")
}

/// Structured single-turn call against Gemini. Mirrors `ollama::chat_json`:
/// returns parsed JSON. `vision` selects the OCR-tuned model and lets images
/// through. `format`, when present, is sent as an OpenAI `json_schema` so the
/// model is shape-constrained; otherwise we ask for a plain JSON object.
pub async fn gemini_chat_json(
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
    vision: bool,
) -> Result<Value> {
    let c = get();
    let key = c
        .gemini_api_key
        .clone()
        .ok_or_else(|| anyhow!("no gemini api key configured"))?;
    let model = if vision { &c.gemini_vision_model } else { &c.gemini_text_model };

    // Build the user message; vision uses the array-content form.
    let user_content: Value = match &images {
        Some(imgs) if !imgs.is_empty() => {
            let mut parts = vec![json!({ "type": "text", "text": user })];
            for img in imgs {
                parts.push(json!({ "type": "image_url", "image_url": { "url": data_url(img) } }));
            }
            json!(parts)
        }
        _ => json!(user),
    };

    let response_format = match &format {
        Some(schema) => json!({
            "type": "json_schema",
            "json_schema": { "name": "extract", "schema": schema }
        }),
        None => json!({ "type": "json_object" }),
    };

    let body = json!({
        "model": model,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_content },
        ],
        "response_format": response_format,
    });

    // A bounded timeout so a wedged network never leaves the UI spinning with no
    // result — a failed connectivity test should fail *visibly*, not hang.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let resp = client
        .post(format!("{GEMINI_BASE}/chat/completions"))
        .bearer_auth(&key)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("gemini {status}: {text}"));
    }

    let v: Value = resp.json().await?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("no choices[0].message.content in gemini response"))?;

    serde_json::from_str(content)
        .map_err(|e| anyhow!("gemini returned non-JSON content: {e}\n---\n{content}"))
}

/// Lightweight connectivity/credential check for the settings "Test" button.
pub async fn test_gemini() -> Result<String> {
    let c = get();
    let model = c.gemini_text_model.clone();
    let out = gemini_chat_json(
        "You are a connectivity check.",
        "Return JSON: {\"ok\": true}",
        None,
        None,
        false,
    )
    .await?;
    if out.get("ok").is_some() || out.is_object() {
        Ok(format!("Connected to Gemini ({model})."))
    } else {
        Err(anyhow!("unexpected response"))
    }
}
