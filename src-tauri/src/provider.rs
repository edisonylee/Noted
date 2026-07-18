// Model-provider routing. noted is local-first ($0, private) by default; the
// "Balanced" mode offloads only the latency-sensitive *structured* calls;
// "Hosted" routes every model-dependent feature through the Noted API.
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
// Gemini speaks an OpenAI-compatible dialect at this base (chat/completions).
const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com/v1";
// Anthropic has no OpenAI-compatible dialect — raw Messages API over reqwest
// (there is no official Rust SDK).
const ANTHROPIC_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const CONFIG_FILE: &str = "provider.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 100% local Ollama. The default. $0, fully private.
    Local,
    /// Local embeddings + chat, but extract/OCR go to the cloud (fast + cheap).
    Balanced,
    /// No local model requirement: chat, vision, embeddings, and ASR use the Noted API.
    Hosted,
    /// User-supplied provider credentials, independently selected by capability.
    Byok,
}

impl Mode {
    fn parse(s: &str) -> Mode {
        match s.to_ascii_lowercase().as_str() {
            "balanced" => Mode::Balanced,
            "hosted" => Mode::Hosted,
            "byok" => Mode::Byok,
            _ => Mode::Local,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    NotedHosted,
    Local,
    Openai,
    Gemini,
    Anthropic,
    OpenaiCompatible,
    Groq,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityChoice {
    pub provider: ProviderId,
    pub model: String,
    #[serde(default)]
    pub base_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ByokConfig {
    #[serde(default = "d_byok_intelligence")]
    pub intelligence: CapabilityChoice,
    #[serde(default = "d_byok_vision")]
    pub vision: CapabilityChoice,
    #[serde(default = "d_byok_embeddings")]
    pub embeddings: CapabilityChoice,
    #[serde(default = "d_byok_transcription")]
    pub transcription: CapabilityChoice,
    #[serde(default = "d_byok_speech")]
    pub speech: CapabilityChoice,
}

fn choice(provider: ProviderId, model: &str, base_url: &str) -> CapabilityChoice {
    CapabilityChoice { provider, model: model.into(), base_url: base_url.into() }
}
fn d_byok_intelligence() -> CapabilityChoice {
    choice(ProviderId::Openai, "gpt-5-mini", OPENAI_DEFAULT_BASE)
}
fn d_byok_vision() -> CapabilityChoice {
    choice(ProviderId::Openai, "gpt-5-mini", OPENAI_DEFAULT_BASE)
}
fn d_byok_embeddings() -> CapabilityChoice {
    choice(ProviderId::Openai, "text-embedding-3-small", OPENAI_DEFAULT_BASE)
}
fn d_byok_transcription() -> CapabilityChoice {
    choice(ProviderId::Openai, "gpt-4o-mini-transcribe", OPENAI_DEFAULT_BASE)
}
fn d_byok_speech() -> CapabilityChoice {
    choice(ProviderId::System, "macos", "")
}
impl Default for ByokConfig {
    fn default() -> Self {
        Self {
            intelligence: d_byok_intelligence(),
            vision: d_byok_vision(),
            embeddings: d_byok_embeddings(),
            transcription: d_byok_transcription(),
            speech: d_byok_speech(),
        }
    }
}

pub fn embedding_fingerprint(choice: &CapabilityChoice) -> String {
    let base_class = match choice.provider {
        ProviderId::OpenaiCompatible => choice.base_url.trim_end_matches('/'),
        ProviderId::Openai => "openai",
        ProviderId::Gemini => "gemini",
        ProviderId::NotedHosted => "noted_hosted",
        _ => "unsupported",
    };
    format!("{:?}|{}|{}|768|provider-default", choice.provider, base_class, choice.model)
}

/// Which cloud service Balanced mode's extract/OCR calls hit. "openai" means
/// any OpenAI-compatible endpoint (OpenAI itself, LM Studio, llama.cpp server,
/// vLLM, OpenRouter…) via a configurable base URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudProvider {
    Gemini,
    OpenAI,
    Anthropic,
}

impl CloudProvider {
    fn parse(s: &str) -> CloudProvider {
        match s.to_ascii_lowercase().as_str() {
            "openai" => CloudProvider::OpenAI,
            "anthropic" => CloudProvider::Anthropic,
            _ => CloudProvider::Gemini,
        }
    }
    fn keychain_account(self) -> &'static str {
        match self {
            CloudProvider::Gemini => "gemini_api_key",
            CloudProvider::OpenAI => "openai_api_key",
            CloudProvider::Anthropic => "anthropic_api_key",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "config_version")]
    pub version: u32,
    pub mode: Mode,
    #[serde(default)]
    pub byok: ByokConfig,
    /// Local Ollama models for the text/vision paths — any pulled model works
    /// (defaults are the tuned qwen2.5 pair). The embedding model is NOT
    /// configurable: the vec0 tables are 768-dim and a different embedder is a
    /// different, incompatible vector space (switching would require a full
    /// re-embed of every note and entity).
    #[serde(default = "d_text_model")]
    pub text_model: String,
    #[serde(default = "d_vision_model")]
    pub vision_model: String,
    /// Which cloud service Balanced routes extract/OCR to.
    #[serde(default = "d_cloud_provider")]
    pub cloud_provider: CloudProvider,
    pub gemini_text_model: String,   // extraction hot path — cheapest/fastest
    pub gemini_vision_model: String, // handwriting OCR — a touch stronger
    /// OpenAI-compatible endpoint (OpenAI, LM Studio, llama.cpp, OpenRouter…).
    #[serde(default = "d_openai_base")]
    pub openai_base_url: String,
    #[serde(default = "d_openai_text")]
    pub openai_text_model: String,
    #[serde(default = "d_openai_vision")]
    pub openai_vision_model: String,
    /// Anthropic model ids — mirrors the Gemini split: a fast/cheap model for
    /// the per-note extract hot path, a stronger one for handwriting OCR.
    #[serde(default = "d_anthropic_text")]
    pub anthropic_text_model: String,
    #[serde(default = "d_anthropic_vision")]
    pub anthropic_vision_model: String,
    // Loaded from the Keychain at startup; never serialized to the JSON file.
    #[serde(skip)]
    pub gemini_api_key: Option<String>,
    #[serde(skip)]
    pub openai_api_key: Option<String>,
    #[serde(skip)]
    pub anthropic_api_key: Option<String>,
    #[serde(skip)]
    pub groq_api_key: Option<String>,
    #[serde(skip)]
    pub openai_compatible_api_key: Option<String>,
}

fn d_text_model() -> String {
    crate::ollama::TEXT_MODEL.into()
}
fn config_version() -> u32 { 2 }
fn d_vision_model() -> String {
    crate::ollama::VISION_MODEL.into()
}
fn d_cloud_provider() -> CloudProvider {
    CloudProvider::Gemini
}
fn d_openai_base() -> String {
    OPENAI_DEFAULT_BASE.into()
}
fn d_openai_text() -> String {
    "gpt-5-mini".into()
}
fn d_openai_vision() -> String {
    "gpt-5-mini".into()
}
fn d_anthropic_text() -> String {
    "claude-haiku-4-5".into()
}
fn d_anthropic_vision() -> String {
    "claude-sonnet-5".into()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: config_version(),
            mode: Mode::Local,
            byok: ByokConfig::default(),
            text_model: d_text_model(),
            vision_model: d_vision_model(),
            cloud_provider: d_cloud_provider(),
            gemini_text_model: "gemini-2.5-flash-lite".into(),
            gemini_vision_model: "gemini-2.5-flash".into(),
            openai_base_url: d_openai_base(),
            openai_text_model: d_openai_text(),
            openai_vision_model: d_openai_vision(),
            anthropic_text_model: d_anthropic_text(),
            anthropic_vision_model: d_anthropic_vision(),
            gemini_api_key: None,
            openai_api_key: None,
            anthropic_api_key: None,
            groq_api_key: None,
            openai_compatible_api_key: None,
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

/// The API key for the active cloud provider, if one is stored.
fn active_cloud_key(c: &Config) -> Option<&str> {
    match c.cloud_provider {
        CloudProvider::Gemini => c.gemini_api_key.as_deref(),
        CloudProvider::OpenAI => c.openai_api_key.as_deref(),
        CloudProvider::Anthropic => c.anthropic_api_key.as_deref(),
    }
    .filter(|k| !k.is_empty())
}

/// True when structured calls (extract + OCR) should be routed to the cloud.
pub fn use_cloud_for_extract() -> bool {
    let c = get();
    c.mode == Mode::Balanced && active_cloud_key(&c).is_some()
}

pub fn use_hosted() -> bool {
    get().mode == Mode::Hosted && crate::hosted::has_key()
}

pub fn use_byok() -> bool {
    get().mode == Mode::Byok
}

fn provider_key(c: &Config, provider: ProviderId) -> Result<String> {
    let key = match provider {
        ProviderId::Openai => c.openai_api_key.as_deref(),
        ProviderId::Gemini => c.gemini_api_key.as_deref(),
        ProviderId::Anthropic => c.anthropic_api_key.as_deref(),
        ProviderId::Groq => c.groq_api_key.as_deref(),
        ProviderId::OpenaiCompatible => c.openai_compatible_api_key.as_deref(),
        _ => None,
    };
    key.filter(|k| !k.is_empty()).map(str::to_string)
        .ok_or_else(|| anyhow!("no credential configured for {:?}", provider))
}

fn openai_base(choice: &CapabilityChoice) -> Result<&str> {
    match choice.provider {
        ProviderId::Openai => Ok(OPENAI_DEFAULT_BASE),
        ProviderId::Gemini => Ok(GEMINI_BASE),
        ProviderId::Groq => Ok("https://api.groq.com/openai/v1"),
        ProviderId::OpenaiCompatible => {
            let base = choice.base_url.trim_end_matches('/');
            if !base.starts_with("https://") && !base.starts_with("http://127.0.0.1") && !base.starts_with("http://localhost") {
                return Err(anyhow!("custom provider base URL must use HTTPS or loopback HTTP"));
            }
            Ok(base)
        }
        _ => Err(anyhow!("provider is not OpenAI-compatible")),
    }
}

pub async fn byok_chat_json(
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
) -> Result<Value> {
    let c = get();
    let vision = images.as_ref().is_some_and(|v| !v.is_empty());
    let choice = if vision { &c.byok.vision } else { &c.byok.intelligence };
    match choice.provider {
        ProviderId::Anthropic => {
            anthropic_chat_json(&provider_key(&c, choice.provider)?, &choice.model, system, user, images, format).await
        }
        ProviderId::NotedHosted => crate::hosted::chat_json(system, user, images, format).await,
        ProviderId::Openai | ProviderId::Gemini | ProviderId::Groq | ProviderId::OpenaiCompatible => {
            openai_compat_chat_json(
                openai_base(choice)?,
                &provider_key(&c, choice.provider)?,
                &choice.model,
                system,
                user,
                images,
                format,
            ).await
        }
        _ => Err(anyhow!("selected provider cannot generate structured output")),
    }
}

pub async fn byok_chat_text(system: &str, user: &str) -> Result<String> {
    byok_chat_messages(
        vec![json!({"role":"system","content":system}), json!({"role":"user","content":user})],
        0.2,
    ).await
}

pub async fn byok_chat_messages(messages: Vec<Value>, temperature: f32) -> Result<String> {
    let c = get();
    let choice = &c.byok.intelligence;
    match choice.provider {
        ProviderId::NotedHosted => crate::hosted::chat_messages(messages, temperature).await,
        ProviderId::Anthropic => anthropic_chat_messages(
            &provider_key(&c, choice.provider)?, &choice.model, messages, temperature,
        ).await,
        ProviderId::Openai | ProviderId::Gemini | ProviderId::Groq | ProviderId::OpenaiCompatible => {
            openai_compat_chat_messages(
                openai_base(choice)?, &provider_key(&c, choice.provider)?, &choice.model,
                messages, temperature,
            ).await
        }
        _ => Err(anyhow!("selected intelligence provider cannot generate text")),
    }
}

pub async fn byok_embed(input: &str) -> Result<Vec<f32>> {
    let c = get();
    let choice = &c.byok.embeddings;
    match choice.provider {
        ProviderId::NotedHosted => crate::hosted::embed(input).await,
        ProviderId::Gemini => gemini_embed(&provider_key(&c, choice.provider)?, &choice.model, input).await,
        ProviderId::Openai | ProviderId::OpenaiCompatible => openai_compat_embed(
            openai_base(choice)?, &provider_key(&c, choice.provider)?, &choice.model, input,
        ).await,
        _ => Err(anyhow!("selected provider does not support compatible embeddings")),
    }
}

/// Transcribe one closed 16 kHz mono PCM chunk. Meeting capture/VAD remains
/// local; only this bounded ASR operation is sent to the selected provider.
pub fn byok_transcribe_blocking(samples: &[f32], vocabulary: &[String]) -> Result<String> {
    let c = get();
    let choice = &c.byok.transcription;
    if choice.provider == ProviderId::NotedHosted {
        return crate::hosted::Session::open(vocabulary.to_vec())?.transcribe(samples);
    }
    if choice.provider == ProviderId::Gemini {
        return gemini_transcribe_blocking(&provider_key(&c, choice.provider)?, &choice.model, samples, vocabulary);
    }
    if !matches!(choice.provider, ProviderId::Openai | ProviderId::Groq | ProviderId::OpenaiCompatible) {
        return Err(anyhow!("selected provider does not support transcription"));
    }
    let base = openai_base(choice)?;
    let file = reqwest::blocking::multipart::Part::bytes(crate::hosted::wav_bytes(samples))
        .file_name("speech.wav").mime_str("audio/wav")?;
    let mut form = reqwest::blocking::multipart::Form::new()
        .part("file", file).text("model", choice.model.clone()).text("language", "en");
    if !vocabulary.is_empty() {
        form = form.text("prompt", vocabulary.join(", "));
    }
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90)).build()?
        .post(format!("{base}/audio/transcriptions"))
        .bearer_auth(provider_key(&c, choice.provider)?)
        .multipart(form).send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("transcription provider {status}: {text}"));
    }
    let v: Value = resp.json()?;
    v.get("text").and_then(Value::as_str).map(str::to_string)
        .ok_or_else(|| anyhow!("no text in transcription response"))
}

fn gemini_transcribe_blocking(key: &str, model: &str, samples: &[f32], vocabulary: &[String]) -> Result<String> {
    use base64::Engine;
    let audio = base64::engine::general_purpose::STANDARD.encode(crate::hosted::wav_bytes(samples));
    let vocab = if vocabulary.is_empty() { String::new() } else {
        format!(" Preserve these spellings where heard: {}.", vocabulary.join(", "))
    };
    let body = json!({ "contents": [{ "parts": [
        { "text": format!("Transcribe this English audio verbatim. Return only the transcript.{vocab}") },
        { "inline_data": { "mime_type": "audio/wav", "data": audio } }
    ]}]});
    let model = model.strip_prefix("models/").unwrap_or(model);
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90)).build()?
        .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"))
        .header("x-goog-api-key", key).json(&body).send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(anyhow!("gemini transcription {status}: {text}"));
    }
    let v: Value = resp.json()?;
    v.pointer("/candidates/0/content/parts/0/text").and_then(Value::as_str)
        .map(|s| s.trim().to_string()).ok_or_else(|| anyhow!("no transcript in Gemini response"))
}

// ── Keychain (via the macOS `security` CLI) ─────────────────────────────────
fn keychain_read(account: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if key.is_empty() { None } else { Some(key) }
}

fn keychain_write(account: &str, key: &str) -> Result<()> {
    // -U updates the item if it already exists.
    let status = Command::new("security")
        .args(["add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", account, "-w", key])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("failed to write key to keychain"))
    }
}

fn keychain_delete(account: &str) {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account])
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

/// Load config from disk + keys from the Keychain into the process global.
/// Called once at startup.
pub fn init(dir: &Path) {
    let mut c = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default();
    c.gemini_api_key = keychain_read(CloudProvider::Gemini.keychain_account());
    c.openai_api_key = keychain_read(CloudProvider::OpenAI.keychain_account());
    c.anthropic_api_key = keychain_read(CloudProvider::Anthropic.keychain_account());
    c.groq_api_key = keychain_read("groq_api_key");
    c.openai_compatible_api_key = keychain_read("openai_compatible_api_key");
    *cell().write().unwrap() = c;
}

/// A partial settings update from the UI. Model/url fields: Some(non-empty)
/// sets, everything else leaves unchanged. Key fields: Some(non-empty) stores
/// in the Keychain, Some("") deletes, None leaves unchanged.
#[derive(Default)]
pub struct SettingsPatch {
    pub mode: Option<String>,
    pub cloud_provider: Option<String>,
    pub text_model: Option<String>,
    pub vision_model: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_text_model: Option<String>,
    pub gemini_vision_model: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_api_key: Option<String>,
    pub openai_text_model: Option<String>,
    pub openai_vision_model: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub anthropic_text_model: Option<String>,
    pub anthropic_vision_model: Option<String>,
    pub byok: Option<ByokConfig>,
    pub groq_api_key: Option<String>,
    pub openai_compatible_api_key: Option<String>,
}

/// Apply settings changes from the UI: update mode/provider/models, store or
/// clear keys in the Keychain, persist the JSON, and refresh the global.
pub fn update(dir: &Path, patch: SettingsPatch) -> Result<()> {
    let mut c = get();
    if let Some(m) = patch.mode {
        c.mode = Mode::parse(&m);
    }
    if let Some(p) = patch.cloud_provider {
        c.cloud_provider = CloudProvider::parse(&p);
    }
    if let Some(byok) = patch.byok {
        c.byok = byok;
    }
    let set = |dst: &mut String, src: Option<String>| {
        if let Some(v) = src {
            if !v.trim().is_empty() {
                *dst = v.trim().to_string();
            }
        }
    };
    set(&mut c.text_model, patch.text_model);
    set(&mut c.vision_model, patch.vision_model);
    set(&mut c.gemini_text_model, patch.gemini_text_model);
    set(&mut c.gemini_vision_model, patch.gemini_vision_model);
    set(&mut c.openai_base_url, patch.openai_base_url);
    set(&mut c.openai_text_model, patch.openai_text_model);
    set(&mut c.openai_vision_model, patch.openai_vision_model);
    set(&mut c.anthropic_text_model, patch.anthropic_text_model);
    set(&mut c.anthropic_vision_model, patch.anthropic_vision_model);
    let set_key = |slot: &mut Option<String>,
                   account: &str,
                   incoming: Option<String>|
     -> Result<()> {
        match incoming {
            Some(k) if !k.trim().is_empty() => {
                keychain_write(account, k.trim())?;
                *slot = Some(k.trim().to_string());
            }
            Some(_) => {
                keychain_delete(account);
                *slot = None;
            }
            None => {} // unchanged
        }
        Ok(())
    };
    set_key(
        &mut c.gemini_api_key,
        CloudProvider::Gemini.keychain_account(),
        patch.gemini_api_key,
    )?;
    set_key(
        &mut c.openai_api_key,
        CloudProvider::OpenAI.keychain_account(),
        patch.openai_api_key,
    )?;
    set_key(
        &mut c.anthropic_api_key,
        CloudProvider::Anthropic.keychain_account(),
        patch.anthropic_api_key,
    )?;
    set_key(&mut c.groq_api_key, "groq_api_key", patch.groq_api_key)?;
    set_key(
        &mut c.openai_compatible_api_key,
        "openai_compatible_api_key",
        patch.openai_compatible_api_key,
    )?;
    write_config_file(dir, &c);
    *cell().write().unwrap() = c;
    Ok(())
}

// ── Cloud chat (dispatch by provider) ────────────────────────────────────────

fn sniff_mime(b64: &str) -> &'static str {
    // Cheap magic-byte sniff so the mime is right for vision input.
    if b64.starts_with("iVBOR") {
        "image/png"
    } else if b64.starts_with("R0lGOD") {
        "image/gif"
    } else if b64.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

fn data_url(b64: &str) -> String {
    format!("data:{};base64,{b64}", sniff_mime(b64))
}

/// A bounded timeout so a wedged network never leaves the UI spinning with no
/// result — a failed connectivity test should fail *visibly*, not hang.
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// Structured single-turn call against the configured cloud provider. Mirrors
/// `ollama::chat_json`: returns parsed JSON. `vision` selects the OCR-tuned
/// model and lets images through; `format`, when present, shape-constrains
/// the output (natively where the API supports it, by prompt otherwise).
pub async fn cloud_chat_json(
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
    vision: bool,
) -> Result<Value> {
    let c = get();
    match c.cloud_provider {
        CloudProvider::Gemini => {
            let key = c.gemini_api_key.clone().ok_or_else(|| anyhow!("no gemini api key configured"))?;
            let model = if vision { &c.gemini_vision_model } else { &c.gemini_text_model };
            openai_compat_chat_json(GEMINI_BASE, &key, model, system, user, images, format).await
        }
        CloudProvider::OpenAI => {
            let key = c.openai_api_key.clone().ok_or_else(|| anyhow!("no openai api key configured"))?;
            let model = if vision { &c.openai_vision_model } else { &c.openai_text_model };
            let base = c.openai_base_url.trim_end_matches('/');
            openai_compat_chat_json(base, &key, model, system, user, images, format).await
        }
        CloudProvider::Anthropic => {
            let key = c
                .anthropic_api_key
                .clone()
                .ok_or_else(|| anyhow!("no anthropic api key configured"))?;
            let model = if vision { &c.anthropic_vision_model } else { &c.anthropic_text_model };
            anthropic_chat_json(&key, model, system, user, images, format).await
        }
    }
}

/// OpenAI-compatible chat/completions — serves Gemini's OpenAI dialect,
/// OpenAI itself, and any compatible server (LM Studio, llama.cpp, vLLM,
/// OpenRouter) via the configurable base URL.
async fn openai_compat_chat_json(
    base: &str,
    key: &str,
    model: &str,
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
) -> Result<Value> {
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

    let resp = http_client()?
        .post(format!("{base}/chat/completions"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("cloud provider {status}: {text}"));
    }

    let v: Value = resp.json().await?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("no choices[0].message.content in response"))?;

    parse_json_content(content)
}

async fn openai_compat_chat_messages(
    base: &str,
    key: &str,
    model: &str,
    messages: Vec<Value>,
    temperature: f32,
) -> Result<String> {
    let body = json!({
        "model": model,
        "temperature": temperature,
        "messages": messages,
    });
    let resp = http_client()?
        .post(format!("{base}/chat/completions"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("cloud provider {status}: {text}"));
    }
    let v: Value = resp.json().await?;
    v.pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no choices[0].message.content in response"))
}

async fn openai_compat_embed(base: &str, key: &str, model: &str, input: &str) -> Result<Vec<f32>> {
    let body = json!({ "model": model, "input": input, "dimensions": 768 });
    let resp = http_client()?
        .post(format!("{base}/embeddings"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("embedding provider {status}: {text}"));
    }
    let v: Value = resp.json().await?;
    let embedding = v.pointer("/data/0/embedding")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("no data[0].embedding in response"))?
        .iter()
        .map(|n| n.as_f64().map(|x| x as f32).ok_or_else(|| anyhow!("embedding contained a non-number")))
        .collect::<Result<Vec<_>>>()?;
    validate_embedding(embedding)
}

async fn gemini_embed(key: &str, model: &str, input: &str) -> Result<Vec<f32>> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    let body = json!({
        "model": format!("models/{model}"),
        "content": { "parts": [{ "text": input }] },
        "outputDimensionality": 768,
    });
    let resp = http_client()?
        .post(format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:embedContent"))
        .header("x-goog-api-key", key)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("gemini embeddings {status}: {text}"));
    }
    let v: Value = resp.json().await?;
    let embedding = v.pointer("/embedding/values")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("no embedding.values in Gemini response"))?
        .iter()
        .map(|n| n.as_f64().map(|x| x as f32).ok_or_else(|| anyhow!("embedding contained a non-number")))
        .collect::<Result<Vec<_>>>()?;
    validate_embedding(embedding)
}

fn validate_embedding(embedding: Vec<f32>) -> Result<Vec<f32>> {
    if embedding.len() != 768 {
        return Err(anyhow!("embedding model returned {} dimensions; Noted requires 768", embedding.len()));
    }
    Ok(embedding)
}

/// Anthropic Messages API (raw HTTP — no official Rust SDK). Output shaping:
/// the app's extraction schemas are free-form (emergent `data` objects, no
/// additionalProperties constraints), which Anthropic's structured-outputs
/// mode doesn't accept — so the schema is embedded in the system prompt and
/// the JSON is parsed from the text response instead. Claude models are
/// reliable JSON emitters, and pipeline.rs validates every proposal anyway.
async fn anthropic_chat_json(
    key: &str,
    model: &str,
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
) -> Result<Value> {
    let system_full = match &format {
        Some(schema) => format!(
            "{system}\n\nRespond ONLY with a single JSON object conforming to this JSON Schema — \
             no prose, no code fences:\n{schema}"
        ),
        None => format!("{system}\n\nRespond ONLY with a single JSON object — no prose, no code fences."),
    };

    let mut content = Vec::new();
    if let Some(imgs) = &images {
        for img in imgs {
            content.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": sniff_mime(img), "data": img }
            }));
        }
    }
    content.push(json!({ "type": "text", "text": user }));

    let body = json!({
        "model": model,
        "max_tokens": 8192,
        "system": system_full,
        "messages": [{ "role": "user", "content": content }],
    });

    let resp = http_client()?
        .post(format!("{ANTHROPIC_BASE}/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("anthropic {status}: {text}"));
    }

    let v: Value = resp.json().await?;
    // A refusal is a 200 with stop_reason "refusal" — surface it as an error
    // so the caller falls back to the local model instead of parsing nothing.
    if v.get("stop_reason").and_then(|s| s.as_str()) == Some("refusal") {
        return Err(anyhow!("anthropic declined the request (stop_reason: refusal)"));
    }
    let content = v
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow!("no text block in anthropic response"))?;

    parse_json_content(content)
}

async fn anthropic_chat_messages(
    key: &str,
    model: &str,
    messages: Vec<Value>,
    temperature: f32,
) -> Result<String> {
    let mut system = Vec::new();
    let mut turns = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = message.get("content").cloned().unwrap_or_else(|| json!(""));
        if role == "system" {
            if let Some(text) = content.as_str() { system.push(text.to_string()); }
        } else {
            turns.push(json!({ "role": if role == "assistant" { "assistant" } else { "user" }, "content": content }));
        }
    }
    let body = json!({
        "model": model,
        "max_tokens": 8192,
        "temperature": temperature,
        "system": system.join("\n\n"),
        "messages": turns,
    });
    let resp = http_client()?
        .post(format!("{ANTHROPIC_BASE}/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("anthropic {status}: {text}"));
    }
    let v: Value = resp.json().await?;
    v.get("content").and_then(Value::as_array)
        .and_then(|blocks| blocks.iter().find(|b| b.get("type").and_then(Value::as_str) == Some("text")))
        .and_then(|b| b.get("text")).and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no text block in anthropic response"))
}

/// Parse a model's JSON reply, tolerating markdown code fences.
fn parse_json_content(content: &str) -> Result<Value> {
    let t = content.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(t)
        .trim();
    serde_json::from_str(t)
        .map_err(|e| anyhow!("provider returned non-JSON content: {e}\n---\n{content}"))
}

/// Lightweight connectivity/credential check for the settings "Test" button.
pub async fn test_cloud() -> Result<String> {
    let c = get();
    let (name, model) = match c.cloud_provider {
        CloudProvider::Gemini => ("Gemini", c.gemini_text_model.clone()),
        CloudProvider::OpenAI => ("OpenAI-compatible", c.openai_text_model.clone()),
        CloudProvider::Anthropic => ("Anthropic", c.anthropic_text_model.clone()),
    };
    let out = cloud_chat_json(
        "You are a connectivity check.",
        "Return JSON: {\"ok\": true}",
        None,
        None,
        false,
    )
    .await?;
    if out.get("ok").is_some() || out.is_object() {
        Ok(format!("Connected to {name} ({model})."))
    } else {
        Err(anyhow!("unexpected response"))
    }
}

pub async fn test_byok_capabilities() -> Value {
    let intelligence = byok_chat_json(
        "Return only valid JSON.", "Return {\"ok\":true}.", None, None,
    ).await.map(|_| "passed".to_string()).unwrap_or_else(|e| format!("failed: {e}"));
    // One transparent 1x1 PNG: verifies that the selected model accepts image
    // input without uploading personal content.
    let fixture = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
    let vision = byok_chat_json(
        "Return only valid JSON.", "Return {\"ok\":true}.", Some(vec![fixture.into()]), None,
    ).await.map(|_| "passed".to_string()).unwrap_or_else(|e| format!("failed: {e}"));
    let embeddings = byok_embed("Noted capability test").await
        .map(|v| format!("passed ({} dimensions)", v.len()))
        .unwrap_or_else(|e| format!("failed: {e}"));
    let transcription = tauri::async_runtime::spawn_blocking(|| byok_transcribe_blocking(&vec![0.0; 8_000], &[]))
        .await.map_err(|e| anyhow!(e.to_string())).and_then(|r| r)
        .map(|_| "passed".to_string()).unwrap_or_else(|e| format!("failed: {e}"));
    json!({ "intelligence": intelligence, "vision": vision, "embeddings": embeddings, "transcription": transcription })
}

#[cfg(test)]
mod byok_tests {
    use super::*;

    #[test]
    fn legacy_config_migrates_with_byok_defaults() {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value.as_object_mut().unwrap().remove("byok");
        let c: Config = serde_json::from_value(value).unwrap();
        assert_eq!(c.byok.intelligence.provider, ProviderId::Openai);
        assert_eq!(c.byok.embeddings.model, "text-embedding-3-small");
        assert_eq!(c.byok.speech.provider, ProviderId::System);
    }

    #[test]
    fn serialized_config_never_contains_credentials() {
        let mut c = Config::default();
        c.openai_api_key = Some("secret-openai".into());
        c.groq_api_key = Some("secret-groq".into());
        let text = serde_json::to_string(&c).unwrap();
        assert!(!text.contains("secret-openai"));
        assert!(!text.contains("secret-groq"));
        assert!(!text.contains("api_key"));
    }

    #[test]
    fn compatible_url_requires_https_or_loopback() {
        let mut c = choice(ProviderId::OpenaiCompatible, "model", "http://example.com/v1");
        assert!(openai_base(&c).is_err());
        c.base_url = "http://localhost:1234/v1/".into();
        assert_eq!(openai_base(&c).unwrap(), "http://localhost:1234/v1");
        c.base_url = "https://models.example/v1/".into();
        assert_eq!(openai_base(&c).unwrap(), "https://models.example/v1");
    }

    #[test]
    fn embedding_dimensions_are_enforced() {
        assert!(validate_embedding(vec![0.0; 767]).is_err());
        assert_eq!(validate_embedding(vec![0.0; 768]).unwrap().len(), 768);
    }

    #[test]
    fn fingerprint_changes_with_model_and_provider() {
        let a = choice(ProviderId::Openai, "a", "");
        let b = choice(ProviderId::Openai, "b", "");
        let c = choice(ProviderId::Gemini, "a", "");
        assert_ne!(embedding_fingerprint(&a), embedding_fingerprint(&b));
        assert_ne!(embedding_fingerprint(&a), embedding_fingerprint(&c));
    }
}
