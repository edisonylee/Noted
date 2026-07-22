// Meeting recorder: local-first Granola. Capture (mic + system-audio tap, two
// streams → deterministic Me/Them), live whisper transcription, template-driven
// local summarization. See MEETINGS_PLAN.md for the full design.
//
// Lifecycle: Idle → Recording (start) → Summarizing (stop) → Done. One meeting
// at a time; state lives here (managed by Tauri), rows live in db.rs tables.

pub mod asr;
pub mod capture;
pub mod detect;
pub mod diarize;
pub mod pdf;
pub mod store;
pub mod summarize;
pub mod video;

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
    /// True from the moment stop() is accepted until the drain finishes. The
    /// slot stays occupied for that whole window (whisper on the tail can take
    /// a minute) so a prompt click can't start a second tap/mic session over
    /// one that is still tearing down.
    pub stopping: bool,
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
    /// Domain terms whisper keeps mishearing (company names, products,
    /// acronyms — "a16z"). Fed to the decoder as a bias and used to
    /// canonicalize near-miss spellings after decode.
    #[serde(default)]
    pub vocabulary: Vec<String>,
    /// "whisper", "parakeet", or "hosted". Hosted uses the scoped Noted API
    /// key in macOS Keychain and keeps local engines as offline fallbacks.
    #[serde(default = "d_engine")]
    pub asr_engine: String,
    /// macOS voice-processing (AEC) on the mic: the OS subtracts what the
    /// speakers play from the mic signal, so the other side of a call never
    /// lands on the "me" channel. Off = raw cpal mic.
    #[serde(default = "d_true")]
    pub mic_aec: bool,
    /// Record the meeting app's WINDOW as video (ScreenCaptureKit, macOS 15+;
    /// follows the window even when covered or on another Space). Needs the
    /// one-time Screen Recording permission.
    #[serde(default = "d_true")]
    pub record_video: bool,
    /// Days to keep window videos before the launch-time sweep deletes them
    /// (transcripts and summaries are kept forever). 0 = keep forever.
    #[serde(default = "d_video_days")]
    pub video_keep_days: i64,
}

fn d_video_days() -> i64 {
    14
}

fn d_true() -> bool {
    true
}
fn d_template() -> String {
    store::DEFAULT_TEMPLATE.to_string()
}
fn d_engine() -> String {
    "whisper".into()
}

/// Dictation, recording, and voice-assistant apps that hold the mic without
/// being a meeting (the superwhisper class of false positives).
pub const ALWAYS_IGNORED_BUNDLES: &[&str] = &["corespeech", "replayd"];

pub fn default_ignore() -> Vec<String> {
    [
        "corespeech",
        "replayd",
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
            vocabulary: Vec::new(),
            asr_engine: d_engine(),
            mic_aec: true,
            record_video: crate::release_profile::video_capture(),
            video_keep_days: d_video_days(),
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

fn apply_release_profile(mut value: MeetingsCfg) -> MeetingsCfg {
    if !crate::release_profile::noted_hosted() && value.asr_engine == "hosted" {
        value.asr_engine = d_engine();
    }
    if !crate::release_profile::video_capture() {
        value.record_video = false;
    }
    value
}

fn validate_asr_engine(engine: &str, hosted_ready: bool) -> Result<()> {
    if !matches!(engine, "whisper" | "parakeet" | "hosted") {
        return Err(anyhow!("unknown meeting transcription engine: {engine}"));
    }
    if engine == "hosted" && !hosted_ready {
        return Err(anyhow!(
            "Hosted transcription is not activated on this Mac. Restore the Noted activation credential or choose Whisper."
        ));
    }
    Ok(())
}

pub fn cfg_init(dir: &std::path::Path) {
    if let Ok(text) = std::fs::read_to_string(dir.join("meetings.json")) {
        if let Ok(loaded) = serde_json::from_str::<MeetingsCfg>(&text) {
            *cfg_cell().write().unwrap() = apply_release_profile(loaded);
        }
    }
}

pub fn cfg_update(dir: &std::path::Path, new_cfg: MeetingsCfg) -> Result<()> {
    let new_cfg = apply_release_profile(new_cfg);
    validate_asr_engine(&new_cfg.asr_engine, crate::hosted::has_key())?;
    std::fs::write(
        dir.join("meetings.json"),
        serde_json::to_string_pretty(&new_cfg)?,
    )?;
    *cfg_cell().write().unwrap() = new_cfg;
    Ok(())
}

/// Make sure macOS has granted microphone access before opening Core Audio.
/// An existing grant returns immediately; the system request is made only
/// when the user has never answered the permission prompt. Core Audio can
/// otherwise start successfully while delivering only zero-valued samples,
/// which looks like a healthy recording until the transcript is empty.
#[cfg(target_os = "macos")]
pub async fn ensure_mic_permission() -> Result<()> {
    use cidre::av;

    let media_type = av::MediaType::audio();
    match av::CaptureDevice::authorization_status_for_media_type(media_type)
        .map_err(|e| anyhow!("could not read microphone permission: {e:?}"))?
    {
        av::AuthorizationStatus::Authorized => return Ok(()),
        av::AuthorizationStatus::Denied | av::AuthorizationStatus::Restricted => {
            return Err(anyhow!(
                "microphone access is off — enable noted in System Settings → Privacy & Security → Microphone"
            ));
        }
        av::AuthorizationStatus::NotDetermined => {}
    }

    // Core Audio itself does not register a TCC request, so use
    // AVCaptureDevice for the one-time system prompt.
    let granted_rx = {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut tx = Some(tx);
        let mut completion = cidre::blocks::SendBlock::new1(move |granted: bool| {
            if let Some(tx) = tx.take() {
                let _ = tx.send(granted);
            }
        });
        av::CaptureDevice::request_access_for_media_type_ch(
            media_type,
            &mut completion,
        )
        .map_err(|e| anyhow!("could not request microphone permission: {e:?}"))?;
        rx
    };
    match granted_rx.await {
        Ok(true) => Ok(()),
        Ok(false) => Err(anyhow!(
            "microphone access is off — enable noted in System Settings → Privacy & Security → Microphone"
        )),
        Err(_) => Err(anyhow!(
            "macOS closed the microphone permission request before it completed"
        )),
    }
}

#[cfg(not(target_os = "macos"))]
pub async fn ensure_mic_permission() -> Result<()> {
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

/// Parakeet-TDT 0.6B v2 (int8 sherpa-onnx export): the four files that make
/// up the model, downloaded individually (no archive handling).
pub const PARAKEET_DIR: &str = "parakeet-tdt-0.6b-v2-int8";
pub const PARAKEET_FILES: &[&str] = &[
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

pub fn parakeet_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("{e}"))?
        .join("models")
        .join(PARAKEET_DIR))
}

pub fn parakeet_ready(app: &tauri::AppHandle) -> bool {
    parakeet_dir(app)
        .map(|d| PARAKEET_FILES.iter().all(|f| d.join(f).exists()))
        .unwrap_or(false)
}

/// Resolve the configured ASR engine against what's actually downloaded.
/// Parakeet with missing files degrades to whisper (recording must never
/// fail over a settings/download mismatch).
pub fn engine_spec(app: &tauri::AppHandle) -> Result<asr::EngineSpec> {
    if crate::provider::use_byok() {
        return Ok(asr::EngineSpec::Byok {
            vocabulary: cfg().vocabulary,
        });
    }
    let config = cfg();
    validate_asr_engine(&config.asr_engine, crate::hosted::has_key())?;
    if crate::release_profile::noted_hosted() && config.asr_engine == "hosted" {
        return Ok(asr::EngineSpec::Hosted {
            vocabulary: config.vocabulary,
        });
    }
    if config.asr_engine == "parakeet" {
        if parakeet_ready(app) {
            return Ok(asr::EngineSpec::Parakeet {
                dir: parakeet_dir(app)?,
            });
        }
        eprintln!("[noted] parakeet selected but not downloaded — using whisper");
    }
    Ok(asr::EngineSpec::Whisper {
        model: model_path(app)?,
    })
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
    let engine = engine_spec(app)?; // fail fast before any DB writes
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

    // ASR vocabulary bias: this meeting's attendees plus the user's custom
    // terms. Names remembered from unrelated meetings must not bias Whisper.
    let (asr_hint, vocab) = {
        let c = cfg();
        let names = event_json
            .as_ref()
            .map(summarize::external_attendees)
            .unwrap_or_default();
        (asr::vocab_hint(&names, &c.vocabulary), c.vocabulary)
    };

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
    let started_epoch_ms = epoch_ms();
    let mut threads = Vec::new();

    // ASR and retained-audio writers must be ready before capture starts or the
    // UI announces a recording. Previously this initialization happened in
    // the background, so a rejected Hosted session left a convincing but empty
    // meeting row running until the user pressed Stop.
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let asr_thread = {
        let args = asr::WorkerArgs {
            meeting_id: id,
            me: me.clone(),
            them: them.clone(),
            stop: stop.clone(),
            engine,
            ready: ready_tx,
            audio_dir: audio_dir.clone(),
            started_epoch_ms,
            speaker_model: if crate::release_profile::diarization() {
                diarize::model_path(app)
            } else {
                None
            },
            asr_hint,
            vocab,
        };
        let h = app.clone();
        std::thread::spawn(move || asr::run_worker(h, args))
    };
    match ready_rx.recv() {
        Ok(Ok(())) => threads.push(asr_thread),
        Ok(Err(message)) => {
            let _ = asr_thread.join();
            let db = app.state::<Db>();
            let conn = db.0.lock().unwrap();
            let _ = store::delete_meeting(&conn, id);
            drop(conn);
            if let Some(dir) = &audio_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(anyhow!(message));
        }
        Err(_) => {
            let _ = asr_thread.join();
            let db = app.state::<Db>();
            let conn = db.0.lock().unwrap();
            let _ = store::delete_meeting(&conn, id);
            drop(conn);
            if let Some(dir) = &audio_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(anyhow!("meeting transcription stopped during startup"));
        }
    }

    if capture::tap_supported() {
        let (b, s) = (them.clone(), stop.clone());
        let log = audio_dir.as_ref().map(|d| d.join("capture.log"));
        threads.push(std::thread::spawn(move || {
            capture::run_system_tap(b, s, log)
        }));
    } else {
        eprintln!("[noted] system-audio tap needs macOS 14.4+; recording mic only");
    }
    {
        let (b, s) = (me.clone(), stop.clone());
        let aec = cfg().mic_aec;
        let log = audio_dir.as_ref().map(|d| d.join("capture.log"));
        threads.push(std::thread::spawn(move || capture::run_mic(b, s, aec, log)));
    }
    // Window video rides along when enabled (its own dir derivation — audio
    // retention off shouldn't disable video). Fire-and-forget: the worker
    // stamps video_path itself; stop() doesn't wait on it.
    if crate::release_profile::video_capture() && cfg().record_video {
        if let Ok(base) = app.path().app_data_dir() {
            video::spawn(
                app.clone(),
                id,
                base.join("meetings").join(id.to_string()),
                stop.clone(),
                source_bundle.clone(),
                cfg().ignore_bundles,
            );
        }
    }

    *guard = Some(Active {
        id,
        title: title.clone(),
        started_epoch_ms,
        stop,
        threads,
        me,
        them,
        audio_dir,
        source_bundle,
        event_end_min,
        event_date,
        stopping: false,
    });
    detect::close_prompt(app); // an accepted (or now-moot) prompt goes away
    let _ = app.emit(
        "meeting-started",
        json!({ "meetingId": id, "title": title }),
    );
    Ok(id)
}

/// Stop the active meeting: join capture/ASR (flushes the final segments),
/// stamp the row, then summarize in the background with the default template.
pub async fn stop(app: tauri::AppHandle) -> Result<Option<i64>> {
    // Mark stopping but leave the slot occupied until the drain completes:
    // start() keeps refusing and detect keeps treating us as recording, so a
    // repeated prompt/join click can't spawn a second concurrent capture.
    let (id, title, threads, audio_dir, stop_flag) = {
        let state = app.state::<MeetingState>();
        let mut guard = state.0.lock().unwrap();
        let Some(active) = guard.as_mut() else {
            return Ok(None);
        };
        if active.stopping {
            return Ok(None); // a second stop while the first is draining
        }
        active.stopping = true;
        (
            active.id,
            active.title.clone(),
            std::mem::take(&mut active.threads),
            active.audio_dir.clone(),
            active.stop.clone(),
        )
    };
    stop_flag.store(true, Ordering::Relaxed);

    // Joins block (worker drains + transcribes the tail) — off the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        for t in threads {
            let _ = t.join();
        }
    })
    .await
    .map_err(|e| anyhow!("join: {e}"))?;
    {
        let state = app.state::<MeetingState>();
        let mut guard = state.0.lock().unwrap();
        if guard.as_ref().map_or(false, |a| a.id == id) {
            *guard = None;
        }
    }

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

    // Auto-enhance when the call ends. Speaker identities remain anonymous
    // until the user labels them for this meeting.
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        match summarize::run(&h, id, None).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("[noted] meeting summarize failed: {e}");
                let db = h.state::<Db>();
                let conn = db.0.lock().unwrap();
                // Capture already succeeded: preserve the meeting as a usable
                // transcript even when optional note generation fails.
                let _ = store::set_status(&conn, id, "done");
                drop(conn);
                let _ = h.emit(
                    "meeting-summarized",
                    json!({ "meetingId": id, "summaryFailed": true }),
                );
                detect::show_status_card(
                    &h,
                    &title,
                    "Meeting saved — transcript ready; notes couldn't be generated.",
                );
            }
        }
    });
    Ok(Some(id))
}

/// Recovery diarization for one interrupted meeting (labels are written only
/// by the live stop path, which a crash never reaches): rebuild them from the
/// retained wall-anchored WAV so a killed app can't leave a multi-speaker
/// meeting reading "Them" forever.
fn rediarize_interrupted(app: &tauri::AppHandle, id: i64) {
    if !crate::release_profile::diarization() {
        return;
    }
    let Some(model) = diarize::model_path(app) else {
        return;
    };
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    let dir = data_dir.join("meetings").join(id.to_string());
    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    match asr::rediarize_from_wav(&conn, &model, &dir, id, false) {
        Ok(n) if n > 0 => {
            println!("[noted] recovered speaker labels for meeting {id} ({n} voices)")
        }
        Ok(_) => {}
        Err(e) => eprintln!("[noted] recovery diarization failed for {id}: {e}"),
    }
}

/// Startup reconciliation: a crash or dev-rebuild mid-recording leaves rows
/// stuck in 'recording'/'summarizing' forever. Anything with a transcript
/// gets its speaker labels rebuilt from the retained audio, then summarized;
/// empty ones are marked failed.
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
            // Labels first (blocking: onnx over the whole WAV), so the
            // summary and the reloaded transcript see speaker names.
            if crate::release_profile::diarization() {
                let h2 = h.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    rediarize_interrupted(&h2, id);
                })
                .await;
            }
            match summarize::run(&h, id, None).await {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("[noted] recovery summarize failed for {id}: {e}");
                    let db = h.state::<Db>();
                    let conn = db.0.lock().unwrap();
                    let _ = store::set_status(&conn, id, "done");
                    drop(conn);
                    let _ = h.emit(
                        "meeting-summarized",
                        json!({ "meetingId": id, "summaryFailed": true }),
                    );
                }
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
            let sig =
                a.me.last_signal
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
                "stopping": a.stopping,
            })
        }
        None => json!({ "active": false }),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_asr_engine;

    #[test]
    fn hosted_asr_cannot_start_without_activation() {
        assert!(validate_asr_engine("hosted", false).is_err());
        assert!(validate_asr_engine("hosted", true).is_ok());
        assert!(validate_asr_engine("whisper", false).is_ok());
        assert!(validate_asr_engine("bogus", true).is_err());
    }
}
