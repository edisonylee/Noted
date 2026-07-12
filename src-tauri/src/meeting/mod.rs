// Meeting recorder: local-first Granola. Capture (mic + system-audio tap, two
// streams → deterministic Me/Them), live whisper transcription, template-driven
// local summarization. See MEETINGS_PLAN.md for the full design.
//
// Lifecycle: Idle → Recording (start) → Summarizing (stop) → Done. One meeting
// at a time; state lives here (managed by Tauri), rows live in db.rs tables.

pub mod asr;
pub mod capture;
pub mod detect;
pub mod store;
pub mod summarize;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use crate::db::Db;
use capture::ChannelBuf;

pub struct MeetingState(pub Mutex<Option<Active>>);

pub struct Active {
    pub id: i64,
    pub title: String,
    pub started_epoch_ms: u64,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub me: Arc<ChannelBuf>,
    pub them: Arc<ChannelBuf>,
    audio_dir: Option<std::path::PathBuf>,
    /// Bundle id of the app whose mic use triggered this recording (mic-detect
    /// starts only) — auto-stop watches for it releasing the mic.
    pub source_bundle: Option<String>,
    /// Scheduled end (minutes from Eastern midnight) + day, when calendar-born.
    pub event_end_min: Option<i64>,
    pub event_date: Option<String>,
}

// ---------------------------------------------------------------------------
// Config: meetings.json in app data (provider.json pattern — no secrets).
// ---------------------------------------------------------------------------

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct MeetingsCfg {
    /// Master switch for both detection prompts (calendar + mic).
    #[serde(default = "d_true")]
    pub auto_prompt: bool,
    /// Keep per-channel WAVs (transcript verifiability — Granola's top
    /// complaint is that you can't check what was actually said).
    #[serde(default = "d_true")]
    pub retain_audio: bool,
    /// Mic-detect ignore list — matched as case-insensitive substrings of the
    /// bundle id, so "superwhisper" covers any vendor prefix.
    #[serde(default = "default_ignore")]
    pub ignore_bundles: Vec<String>,
    #[serde(default = "d_template")]
    pub default_template: String,
}

fn d_true() -> bool {
    true
}
fn d_template() -> String {
    store::DEFAULT_TEMPLATE.to_string()
}

/// Dictation, recording, and voice-assistant apps that hold the mic without
/// being a meeting (the superwhisper class of false positives).
pub fn default_ignore() -> Vec<String> {
    [
        "superwhisper",
        "wispr",
        "voiceink",
        "com.noted.app",
        "obsproject",
        "loom",
        "quicktime",
        "voicememos",
        "com.openai.chat",
        "anthropic",
        "vscode",
        "cursor",
        "dev.warp",
        "raycast",
        "krisp",
        "dictation",
        "controlcenter",
        "systempreferences",
        "com.apple.systemsettings",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Default for MeetingsCfg {
    fn default() -> Self {
        Self {
            auto_prompt: true,
            retain_audio: true,
            ignore_bundles: default_ignore(),
            default_template: d_template(),
        }
    }
}

static CFG: OnceLock<RwLock<MeetingsCfg>> = OnceLock::new();

fn cfg_cell() -> &'static RwLock<MeetingsCfg> {
    CFG.get_or_init(|| RwLock::new(MeetingsCfg::default()))
}

pub fn cfg() -> MeetingsCfg {
    cfg_cell().read().unwrap().clone()
}

pub fn cfg_init(dir: &std::path::Path) {
    if let Ok(text) = std::fs::read_to_string(dir.join("meetings.json")) {
        if let Ok(loaded) = serde_json::from_str::<MeetingsCfg>(&text) {
            *cfg_cell().write().unwrap() = loaded;
        }
    }
}

pub fn cfg_update(dir: &std::path::Path, new_cfg: MeetingsCfg) -> Result<()> {
    std::fs::write(
        dir.join("meetings.json"),
        serde_json::to_string_pretty(&new_cfg)?,
    )?;
    *cfg_cell().write().unwrap() = new_cfg;
    Ok(())
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Meeting transcription prefers the turbo model; falls back to the quick-voice
/// base model so recording works before the big download.
pub fn model_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("{e}"))?
        .join("models");
    let turbo = dir.join("ggml-large-v3-turbo.bin");
    if turbo.exists() {
        return Ok(turbo);
    }
    let base = dir.join("ggml-base.en.bin");
    if base.exists() {
        return Ok(base);
    }
    Err(anyhow!(
        "no whisper model downloaded — open Settings and download the voice model"
    ))
}

/// Begin recording. `event_json` is the calendar-event snapshot when started
/// from Coming Up / a calendar prompt; `source_bundle` is the mic-holding app
/// when started from a mic-detection prompt (drives auto-stop).
pub fn start(
    app: &tauri::AppHandle,
    title: String,
    event_id: Option<String>,
    event_json: Option<Value>,
    retain_audio: bool,
    source_bundle: Option<String>,
) -> Result<i64> {
    let model = model_path(app)?; // fail fast before any DB writes
    let state = app.state::<MeetingState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err(anyhow!("a meeting is already recording"));
    }
    let event_end_min = event_json.as_ref().and_then(|e| e["end_min"].as_i64());
    let event_date = event_json
        .as_ref()
        .and_then(|e| e["date"].as_str())
        .map(String::from);

    let now = chrono::Utc::now().to_rfc3339();
    let id = {
        let db = app.state::<Db>();
        let conn = db.0.lock().unwrap();
        store::create_meeting(
            &conn,
            &title,
            event_id.as_deref(),
            event_json.map(|v| v.to_string()).as_deref(),
            &now,
        )?
    };

    let audio_dir = if retain_audio {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| anyhow!("{e}"))?
            .join("meetings")
            .join(id.to_string());
        Some(dir)
    } else {
        None
    };

    let me = ChannelBuf::new();
    let them = ChannelBuf::new();
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();

    if capture::tap_supported() {
        let (b, s) = (them.clone(), stop.clone());
        threads.push(std::thread::spawn(move || capture::run_system_tap(b, s)));
    } else {
        eprintln!("[noted] system-audio tap needs macOS 14.4+; recording mic only");
    }
    {
        let (b, s) = (me.clone(), stop.clone());
        threads.push(std::thread::spawn(move || capture::run_mic(b, s)));
    }
    {
        let args = asr::WorkerArgs {
            meeting_id: id,
            me: me.clone(),
            them: them.clone(),
            stop: stop.clone(),
            model_path: model,
            audio_dir: audio_dir.clone(),
        };
        let h = app.clone();
        threads.push(std::thread::spawn(move || asr::run_worker(h, args)));
    }

    *guard = Some(Active {
        id,
        title: title.clone(),
        started_epoch_ms: epoch_ms(),
        stop,
        threads,
        me,
        them,
        audio_dir,
        source_bundle,
        event_end_min,
        event_date,
    });
    detect::close_prompt(app); // an accepted (or now-moot) prompt goes away
    let _ = app.emit("meeting-started", json!({ "meetingId": id, "title": title }));
    Ok(id)
}

/// Stop the active meeting: join capture/ASR (flushes the final segments),
/// stamp the row, then summarize in the background with the default template.
pub async fn stop(app: tauri::AppHandle) -> Result<Option<i64>> {
    let active = {
        let state = app.state::<MeetingState>();
        let mut guard = state.0.lock().unwrap();
        guard.take()
    };
    let Some(active) = active else {
        return Ok(None);
    };
    let id = active.id;
    let title = active.title.clone();
    active.stop.store(true, Ordering::Relaxed);

    // Joins block (worker drains + transcribes the tail) — off the async runtime.
    let audio_dir = active.audio_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for t in active.threads {
            let _ = t.join();
        }
    })
    .await
    .map_err(|e| anyhow!("join: {e}"))?;

    let now = chrono::Utc::now().to_rfc3339();
    {
        let db = app.state::<Db>();
        let conn = db.0.lock().unwrap();
        store::set_ended(&conn, id, &now, "summarizing")?;
        if let Some(dir) = &audio_dir {
            let p = |f: &str| dir.join(f).to_string_lossy().to_string();
            store::set_audio_paths(&conn, id, Some(&p("me.wav")), Some(&p("them.wav")))?;
        }
    }
    let _ = app.emit("meeting-stopped", json!({ "meetingId": id }));
    // "Did it end?" should never be a mystery: a small transient card confirms.
    detect::show_status_card(&app, &title, "Meeting saved — writing notes…");

    // Auto-enhance (Granola: enhancement fires when the call ends).
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = summarize::run(&h, id, None).await {
            eprintln!("[noted] meeting summarize failed: {e}");
            let db = h.state::<Db>();
            let conn = db.0.lock().unwrap();
            let _ = store::set_status(&conn, id, "failed");
        }
    });
    Ok(Some(id))
}

/// Startup reconciliation: a crash or dev-rebuild mid-recording leaves rows
/// stuck in 'recording'/'summarizing' forever. Anything with a transcript
/// gets summarized now; empty ones are marked failed.
pub fn reconcile(app: &tauri::AppHandle) {
    let stuck = {
        let db = app.state::<Db>();
        let conn = db.0.lock().unwrap();
        store::list_stuck(&conn).unwrap_or_default()
    };
    let now = chrono::Utc::now().to_rfc3339();
    for (id, segments) in stuck {
        let db = app.state::<Db>();
        let conn = db.0.lock().unwrap();
        if segments == 0 {
            let _ = store::mark_interrupted(&conn, id, &now, "failed");
            continue;
        }
        let _ = store::mark_interrupted(&conn, id, &now, "summarizing");
        drop(conn);
        println!("[noted] recovering interrupted meeting {id} ({segments} segments)");
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = summarize::run(&h, id, None).await {
                eprintln!("[noted] recovery summarize failed for {id}: {e}");
                let db = h.state::<Db>();
                let conn = db.0.lock().unwrap();
                let _ = store::set_status(&conn, id, "failed");
            }
        });
    }
}

/// Live status for polling (phone bridge has no event channel).
pub fn state_json(app: &tauri::AppHandle) -> Value {
    let state = app.state::<MeetingState>();
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(a) => {
            let now = epoch_ms();
            let sig = a
                .me
                .last_signal
                .load(Ordering::Relaxed)
                .max(a.them.last_signal.load(Ordering::Relaxed));
            let signal_ago = if sig == 0 {
                Value::Null
            } else {
                json!(now.saturating_sub(sig))
            };
            json!({
                "active": true,
                "meetingId": a.id,
                "title": a.title,
                "elapsed_ms": now.saturating_sub(a.started_epoch_ms),
                "last_signal_ms_ago": signal_ago,
            })
        }
        None => json!({ "active": false }),
    }
}
