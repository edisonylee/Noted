// Meeting detection — the Granola loop. Two independent signals, both cheap
// and permission-free, feeding one prompt window:
//
//  1. Mic-in-use (ad-hoc calls): poll CoreAudio process objects (macOS 14+)
//     for apps holding the microphone. An app must hold it ≥15s (debounce),
//     not be on the ignore list (dictation apps like superwhisper live there),
//     and not have been prompted in the last 10 minutes. The prompt is titled
//     by attribution ("Huddle detected · Slack").
//  2. Calendar (scheduled meetings): T-60s before any timed event today that
//     looks like a call (join link or ≥2 attendees, never the noted push
//     calendar). One prompt per event id.
//
// An ad-hoc call starting ≤15 min after a scheduled event's start adopts that
// event's title/metadata (Granola's adjacency rule).
//
// The same loop owns auto-stop for the active recording: source app released
// the mic, 15 min of silence, or scheduled end passed with the room quiet.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use super::MeetingState;

const DEBOUNCE_MS: u64 = 15_000;
const COOLDOWN_MS: u64 = 10 * 60_000;
const ADJACENCY_MIN: i64 = 15;
const SILENCE_STOP_MS: u64 = 15 * 60_000;
const CAL_REFRESH_MS: u64 = 300_000;

/// Payload for the currently displayed prompt — the prompt window fetches it
/// on mount (a fresh webview can't receive an event emitted before it loaded).
pub struct PendingPrompt(pub Mutex<Option<Value>>);

/// Cooldowns shared between the loop and the dismiss command.
pub struct DetectState(pub Mutex<HashMap<String, u64>>);

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Friendly (prompt title, app name) for a mic-holding bundle id.
fn attribution(bundle: &str) -> (&'static str, String) {
    let b = bundle.to_lowercase();
    let named = |t: &'static str, n: &str| (t, n.to_string());
    if b.contains("zoom") {
        return named("Meeting detected", "Zoom");
    }
    if b.contains("microsoft.teams") {
        return named("Meeting detected", "Teams");
    }
    if b.contains("slack") {
        return named("Huddle detected", "Slack");
    }
    if b.contains("facetime") {
        return named("Call detected", "FaceTime");
    }
    if b.contains("whatsapp") {
        return named("Call detected", "WhatsApp");
    }
    if b.contains("discord") {
        return named("Call detected", "Discord");
    }
    if b.contains("chrome") {
        return named("Meeting detected", "Chrome");
    }
    if b.contains("safari") {
        return named("Meeting detected", "Safari");
    }
    if b.contains("thebrowser") {
        return named("Meeting detected", "Arc");
    }
    if b.contains("edgemac") {
        return named("Meeting detected", "Edge");
    }
    if b.contains("firefox") {
        return named("Meeting detected", "Firefox");
    }
    if b.contains("brave") {
        return named("Meeting detected", "Brave");
    }
    // Unknown app: last bundle segment, capitalized.
    let tail = bundle.rsplit('.').next().unwrap_or(bundle);
    let mut name: String = tail.to_string();
    if let Some(c) = name.get(0..1) {
        name = c.to_uppercase() + name.get(1..).unwrap_or("");
    }
    ("Call detected", name)
}

/// Bundle ids of processes currently holding the microphone (macOS 14+).
#[cfg(target_os = "macos")]
fn mic_users() -> Vec<String> {
    use cidre::core_audio as ca;
    let Ok(procs) = ca::Process::list() else {
        return Vec::new();
    };
    procs
        .into_iter()
        .filter(|p| p.is_running_input().unwrap_or(false))
        .filter_map(|p| p.bundle_id().ok().map(|b| b.to_string()))
        .filter(|b| !b.is_empty())
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn mic_users() -> Vec<String> {
    Vec::new()
}

fn ignored(bundle: &str, ignore: &[String]) -> bool {
    let b = bundle.to_lowercase();
    ignore.iter().any(|tok| b.contains(&tok.to_lowercase()))
}

/// Today's call-like calendar events (join link or ≥2 attendees; never the
/// noted push calendar).
async fn call_events(app: &tauri::AppHandle) -> Vec<Value> {
    let Ok(dir) = app.path().app_data_dir() else {
        return Vec::new();
    };
    let today = crate::today_local();
    match crate::gcal::events_range(&dir, &today, &today).await {
        Ok(events) => events
            .into_iter()
            .filter(|e| {
                !e["declined"].as_bool().unwrap_or(false)
                    && !e["all_day"].as_bool().unwrap_or(false)
                    && e["start_min"].is_i64()
                    && e["calendar"].as_str().unwrap_or("").to_lowercase() != "noted"
                    && (e["meet_link"].is_string()
                        || e["attendee_count"].as_i64().unwrap_or(0) >= 2)
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn now_min() -> i64 {
    let now = crate::now_eastern();
    (chrono::Timelike::hour(&now) * 60 + chrono::Timelike::minute(&now)) as i64
}

/// Show (or refresh) the record-prompt window with this payload.
fn show_prompt(app: &tauri::AppHandle, payload: Value) {
    {
        let pending = app.state::<PendingPrompt>();
        *pending.0.lock().unwrap() = Some(payload.clone());
    }
    let _ = app.emit("meeting-detected", payload);
    if let Some(w) = app.get_webview_window("record-prompt") {
        let _ = w.show();
        return;
    }
    // Top-right of the primary monitor, Granola-style; never steals focus.
    let (mut x, y) = (60.0, 52.0);
    if let Ok(Some(mon)) = app.primary_monitor() {
        let w = mon.size().width as f64 / mon.scale_factor();
        x = (w - 392.0).max(20.0);
    }
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "record-prompt",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("noted")
    .inner_size(372.0, 132.0)
    .position(x, y)
    .decorations(false)
    .always_on_top(true)
    .resizable(false)
    .skip_taskbar(true)
    .focused(false)
    .build();
}

pub fn close_prompt(app: &tauri::AppHandle) {
    {
        let pending = app.state::<PendingPrompt>();
        *pending.0.lock().unwrap() = None;
    }
    if let Some(w) = app.get_webview_window("record-prompt") {
        let _ = w.close();
    }
}

/// Transient, buttonless top-right card ("Meeting saved — writing notes…").
/// Auto-closes unless a real prompt replaced it meanwhile.
pub fn show_status_card(app: &tauri::AppHandle, meeting_title: &str, message: &str) {
    show_prompt(
        app,
        json!({
            "kind": "status",
            "title": message,
            "app": Value::Null,
            "bundleId": Value::Null,
            "meetingTitle": meeting_title,
            "event": Value::Null,
        }),
    );
    let h = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(6));
        let still_status = {
            let pending = h.state::<PendingPrompt>();
            let guard = pending.0.lock().unwrap();
            guard
                .as_ref()
                .map(|p| p["kind"] == "status")
                .unwrap_or(false)
        };
        if still_status {
            close_prompt(&h);
        }
    });
}

/// Spawn the detection/auto-stop loop (desktop, at startup).
pub fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || run(app));
}

fn run(app: tauri::AppHandle) {
    // bundle → since-when it's been holding the mic
    let mut mic_first_seen: HashMap<String, u64> = HashMap::new();
    // calendar event ids already prompted (today-scoped)
    let mut prompted_events: HashSet<String> = HashSet::new();
    let mut prompted_day = crate::today_local();
    // cached call-like events for today
    let mut events: Vec<Value> = Vec::new();
    let mut events_fetched_at: u64 = 0;
    // auto-stop: when the source app was last seen on the mic
    let mut source_last_seen: u64 = 0;
    // Meetings started manually/from calendar have no source bundle — adopt
    // the first call app seen holding the mic so "left the call" still stops.
    let mut adopted_source: Option<String> = None;
    let mut last_recording: Option<i64> = None;

    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = epoch_ms();
        let cfg = super::cfg();

        // Day rollover clears the per-event prompt memory.
        let today = crate::today_local();
        if today != prompted_day {
            prompted_events.clear();
            prompted_day = today;
        }

        // Refresh the calendar cache every 5 minutes (cheap; local token).
        if cfg.auto_prompt && now.saturating_sub(events_fetched_at) > CAL_REFRESH_MS {
            events = tauri::async_runtime::block_on(call_events(&app));
            events_fetched_at = now;
        }

        let recording_id = {
            let state = app.state::<MeetingState>();
            let guard = state.0.lock().unwrap();
            guard.as_ref().map(|a| a.id)
        };

        let users = mic_users();

        if recording_id != last_recording {
            adopted_source = None;
            source_last_seen = now;
            last_recording = recording_id;
        }

        // ── Auto-stop checks for the active recording ────────────────────
        if recording_id.is_some() {
            let (source, silence_ms, elapsed_ms, sched_end, ev_date) = {
                let state = app.state::<MeetingState>();
                let guard = state.0.lock().unwrap();
                let a = guard.as_ref().unwrap();
                let sig = a
                    .me
                    .last_signal
                    .load(Ordering::Relaxed)
                    .max(a.them.last_signal.load(Ordering::Relaxed));
                (
                    a.source_bundle.clone(),
                    if sig == 0 {
                        now.saturating_sub(a.started_epoch_ms)
                    } else {
                        now.saturating_sub(sig)
                    },
                    now.saturating_sub(a.started_epoch_ms),
                    a.event_end_min,
                    a.event_date.clone(),
                )
            };
            // No known source app? Adopt the first non-ignored one that holds
            // the mic during this recording (that's the call).
            if source.is_none() && adopted_source.is_none() {
                adopted_source = users
                    .iter()
                    .find(|u| !ignored(u, &cfg.ignore_bundles))
                    .cloned();
            }
            let mut stop_reason: Option<&str> = None;
            if silence_ms > SILENCE_STOP_MS {
                stop_reason = Some("15 minutes of silence");
            }
            if let Some(src) = source.as_ref().or(adopted_source.as_ref()) {
                if users.iter().any(|u| u == src) {
                    source_last_seen = now;
                } else if source_last_seen > 0
                    && now.saturating_sub(source_last_seen) > 60_000
                    && elapsed_ms > 60_000
                {
                    stop_reason = Some("call app released the microphone");
                }
            }
            if let (Some(end), Some(date)) = (sched_end, ev_date.as_deref()) {
                if date == crate::today_local()
                    && now_min() > end + 5
                    && silence_ms > 5 * 60_000
                {
                    stop_reason = Some("scheduled end passed");
                }
            }
            if let Some(reason) = stop_reason {
                println!("[noted] auto-stopping meeting: {reason}");
                let h = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = super::stop(h).await {
                        eprintln!("[noted] auto-stop failed: {e}");
                    }
                });
                source_last_seen = 0;
            }
            // While recording, never prompt.
            mic_first_seen.clear();
            continue;
        }
        source_last_seen = now; // reset for the next recording

        if !cfg.auto_prompt {
            mic_first_seen.clear();
            continue;
        }

        // ── Calendar prompt: T-60s before a call-like event ──────────────
        let nmin = now_min();
        for ev in &events {
            let id = ev["id"].as_str().unwrap_or("").to_string();
            let start = ev["start_min"].as_i64().unwrap_or(-1);
            if id.is_empty() || prompted_events.contains(&id) {
                continue;
            }
            if start >= nmin && start - nmin <= 1 {
                prompted_events.insert(id.clone());
                show_prompt(
                    &app,
                    json!({
                        "kind": "calendar",
                        "title": "Meeting starting",
                        "app": Value::Null,
                        "bundleId": Value::Null,
                        "meetingTitle": ev["title"].as_str().unwrap_or("Meeting"),
                        "event": ev,
                    }),
                );
            }
        }

        // ── Self-cleaning prompt: a card nobody answered goes away when it
        // stops being true — the mic-holding app released the mic (call
        // ended), or the calendar start slipped 15+ minutes into the past.
        {
            let pending = app.state::<PendingPrompt>();
            let stale = pending.0.lock().unwrap().as_ref().is_some_and(|p| {
                match p["kind"].as_str() {
                    Some("mic") => p["bundleId"]
                        .as_str()
                        .is_some_and(|b| !users.iter().any(|u| u == b)),
                    Some("calendar") => p["event"]["start_min"]
                        .as_i64()
                        .is_some_and(|start| nmin > start + ADJACENCY_MIN),
                    _ => false,
                }
            });
            if stale {
                close_prompt(&app);
            }
        }

        // ── Mic prompt: an app held the mic ≥15s ─────────────────────────
        let cooldown = app.state::<DetectState>();
        mic_first_seen.retain(|b, _| users.contains(b));
        for bundle in &users {
            if ignored(bundle, &cfg.ignore_bundles) {
                continue;
            }
            let first = *mic_first_seen.entry(bundle.clone()).or_insert(now);
            if now.saturating_sub(first) < DEBOUNCE_MS {
                continue;
            }
            {
                let cd = cooldown.0.lock().unwrap();
                if cd.get(bundle).is_some_and(|t| now.saturating_sub(*t) < COOLDOWN_MS) {
                    continue;
                }
            }
            cooldown.0.lock().unwrap().insert(bundle.clone(), now);

            // Adjacency: adopt a calendar event that started ≤15 min ago.
            let adjacent = events.iter().find(|ev| {
                let start = ev["start_min"].as_i64().unwrap_or(-1);
                let end = ev["end_min"].as_i64().unwrap_or(start + 60);
                start <= nmin && nmin - start <= ADJACENCY_MIN && nmin < end + 5
            });
            let (title, appname) = attribution(bundle);
            let meeting_title = adjacent
                .and_then(|ev| ev["title"].as_str())
                .map(String::from)
                .unwrap_or_else(|| format!("{appname} call"));
            show_prompt(
                &app,
                json!({
                    "kind": "mic",
                    "title": title,
                    "app": appname,
                    "bundleId": bundle,
                    "meetingTitle": meeting_title,
                    "event": adjacent.cloned().unwrap_or(Value::Null),
                }),
            );
            break; // one prompt at a time
        }
    }
}
