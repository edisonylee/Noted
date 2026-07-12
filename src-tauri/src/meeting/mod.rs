// Meeting recorder: local-first Granola. Capture (mic + system-audio tap, two
// streams → deterministic Me/Them), live whisper transcription, template-driven
// local summarization. See MEETINGS_PLAN.md for the full design.
//
// Lifecycle: Idle → Recording (start) → Summarizing (stop) → Done. One meeting
// at a time; state lives here (managed by Tauri), rows live in db.rs tables.

pub mod asr;
pub mod capture;
pub mod store;
pub mod summarize;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
/// from Coming Up / a calendar prompt.
pub fn start(
    app: &tauri::AppHandle,
    title: String,
    event_id: Option<String>,
    event_json: Option<Value>,
    retain_audio: bool,
) -> Result<i64> {
    let model = model_path(app)?; // fail fast before any DB writes
    let state = app.state::<MeetingState>();
    let mut guard = state.0.lock().unwrap();
    if guard.is_some() {
        return Err(anyhow!("a meeting is already recording"));
    }

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
    });
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
