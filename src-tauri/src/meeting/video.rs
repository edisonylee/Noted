// Meeting window video: ScreenCaptureKit records the MEETING APP'S WINDOW as
// an mp4 alongside the audio. The filter is desktop-independent-window, so
// covering the call with another app, minimizing it, or switching Spaces
// never interrupts the recording — it follows the window, not the screen.
//
// Recording uses SCRecordingOutput (macOS 15+, runtime-gated like the audio
// tap): the stream writes H.264 straight to disk, no manual AVAssetWriter
// pumping. Everything here is best-effort — a missing permission, a vanished
// window, or an SC error logs and bows out without touching the meeting.
//
// Retention: window video is the bulkiest artifact a meeting leaves
// (~1-2 MB/min at 10 fps). `video_keep_days` in meetings.json bounds it:
// cleanup_old() at launch deletes expired files and clears their DB paths;
// the transcript and summaries are forever, the pixels are a rolling window.
//
// Permission is never requested from the automatic meeting-start path. The
// Settings UI owns the one explicit request; ordinary recordings only start
// video after preflight says the grant already exists. This prevents macOS
// from presenting the same screen-recording notice on every meeting.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use super::store;
use crate::db::Db;

/// Keep video diagnostics beside the retained meeting artifacts. Video used
/// to report only to process stdout, which made a missing MP4 impossible to
/// explain after the app closed.
pub fn log_status(dir: &Path, message: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("capture.log"))
    {
        let now = chrono::Utc::now().to_rfc3339();
        let _ = writeln!(file, "{now} video: {message}");
    }
}

/// SCRecordingOutput needs macOS 15.0+. Runtime gate, not compile (the app
/// itself supports 14.4 for the audio tap).
#[cfg(target_os = "macos")]
pub fn video_supported() -> bool {
    let ver = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    ver.trim()
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
        >= 15
}

#[cfg(not(target_os = "macos"))]
pub fn video_supported() -> bool {
    false
}

/// Read the current Screen Recording grant without prompting.
#[cfg(target_os = "macos")]
pub fn permission_granted() -> bool {
    cidre::cg::screen_capture_access::preflight()
}

#[cfg(not(target_os = "macos"))]
pub fn permission_granted() -> bool {
    false
}

/// Explicit, user-initiated permission request from Settings. Never call this
/// during meeting start: requesting there is what caused a prompt every time.
#[cfg(target_os = "macos")]
pub fn request_permission() -> bool {
    cidre::cg::screen_capture_access::request()
}

#[cfg(not(target_os = "macos"))]
pub fn request_permission() -> bool {
    false
}

/// Start the window-video worker for a meeting. Returns immediately; the
/// worker looks for the meeting app's window (retrying while the call ramps
/// up), records it until `stop`, then stamps `video_path` and emits
/// `meeting-video-ready`.
pub fn spawn(
    app: tauri::AppHandle,
    meeting_id: i64,
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    source_bundle: Option<String>,
    ignore_bundles: Vec<String>,
) {
    if !video_supported() || !permission_granted() {
        return;
    }
    log_status(&dir, "window recording requested");
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            // cidre's SC objects aren't Send — drive the async API with
            // block_on so everything lives and dies on this one thread
            // (same discipline as the audio capture threads).
            tauri::async_runtime::block_on(async move {
                match macos::record(
                    &app,
                    meeting_id,
                    &dir,
                    &stop,
                    source_bundle,
                    &ignore_bundles,
                )
                .await
                {
                    Ok(Some(path)) => {
                        log_status(&dir, &format!("saved {path}"));
                        {
                            let db = app.state::<Db>();
                            let conn = db.0.lock().unwrap();
                            let _ = store::set_video_path(&conn, meeting_id, Some(&path));
                        }
                        use tauri::Emitter;
                        let _ = app.emit(
                            "meeting-video-ready",
                            serde_json::json!({ "meetingId": meeting_id, "path": path }),
                        );
                        println!("[noted] meeting {meeting_id} window video saved");
                    }
                    Ok(None) => log_status(&dir, "stopped without a saved MP4"),
                    Err(e) => {
                        log_status(&dir, &format!("recorder error: {e}"));
                        eprintln!("[noted] window video unavailable: {e}");
                    }
                }
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, meeting_id, dir, stop, source_bundle, ignore_bundles);
        }
    });
}

/// Delete a meeting's video file and clear its DB path. Used by the manual
/// "delete video" action and the retention sweep.
pub fn delete_video(conn: &rusqlite::Connection, meeting_id: i64, path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = store::set_video_path(conn, meeting_id, None);
}

/// Retention sweep at launch: window videos older than `keep_days` are
/// deleted and their paths cleared. 0 = keep forever.
pub fn cleanup_old(app: &tauri::AppHandle, keep_days: i64) {
    if keep_days <= 0 {
        return;
    }
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(keep_days as u64 * 86_400);
    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    let Ok(rows) = store::meetings_with_video(&conn) else {
        return;
    };
    for (id, path) in rows {
        let expired = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            // A dangling path (file already gone) also gets its row cleared.
            .unwrap_or(true);
        if expired {
            delete_video(&conn, id, &path);
            println!("[noted] expired window video removed (meeting {id})");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use anyhow::{anyhow, Result};
    use cidre::{cm, define_obj_type, ns, objc, sc};

    /// How long to keep looking for the meeting window before giving up —
    /// the app may be opened minutes after recording starts.
    const FIND_WINDOW_MAX_S: u64 = 15 * 60;
    /// Poll cadence while looking for the window / waiting for stop.
    const POLL_MS: u64 = 1_000;
    /// Frames per second — screen-share content, not cinema.
    const FPS: i32 = 10;
    /// Longest output edge in pixels; the window's aspect is preserved.
    const MAX_EDGE: usize = 1_920;

    // SCRecordingOutput's delegate methods are all optional — an empty
    // NSObject subclass satisfies the protocol; failures surface through
    // the file staying at zero bytes instead.
    define_obj_type!(
        RecDelegate + sc::recording_output::DelegateImpl,
        usize,
        NOTED_VIDEO_REC_DELEGATE
    );
    impl sc::recording_output::Delegate for RecDelegate {}
    #[objc::add_methods]
    impl sc::recording_output::DelegateImpl for RecDelegate {}

    /// The meeting app's main window: owned by a candidate bundle, on
    /// screen, layer 0, biggest area. Candidates are the detect-time source
    /// bundle when there is one, else whoever is holding the mic right now.
    fn find_window(
        content: &sc::ShareableContent,
        source_bundle: Option<&str>,
        ignore: &[String],
    ) -> Option<cidre::arc::R<sc::Window>> {
        let candidates: Vec<String> = match source_bundle {
            Some(b) => vec![b.to_string()],
            None => super::super::detect::mic_users()
                .into_iter()
                .filter(|b| {
                    let lb = b.to_lowercase();
                    lb != "com.noted.app"
                        && !super::super::ALWAYS_IGNORED_BUNDLES
                            .iter()
                            .any(|t| lb.contains(t))
                        && !ignore.iter().any(|t| lb.contains(&t.to_lowercase()))
                })
                .collect(),
        };
        if candidates.is_empty() {
            return None;
        }
        let windows = content.windows();
        let mut best: Option<(f64, cidre::arc::R<sc::Window>)> = None;
        for i in 0..windows.len() {
            let w = &windows[i];
            if !w.is_on_screen() || w.window_layer() != 0 {
                continue;
            }
            let Some(app) = w.owning_app() else { continue };
            let bundle = app.bundle_id().to_string();
            if !candidates.iter().any(|c| c == &bundle) {
                continue;
            }
            let f = w.frame();
            let area = f.size.width * f.size.height;
            if area < 40_000.0 {
                continue; // floating toolbars, pip thumbnails
            }
            if best.as_ref().map_or(true, |(a, _)| area > *a) {
                best = Some((area, w.retained()));
            }
        }
        best.map(|(_, w)| w)
    }

    /// Record until `stop`. Ok(None) = never found a window / unsupported;
    /// Ok(Some(path)) = an mp4 landed there.
    pub async fn record(
        _app: &tauri::AppHandle,
        meeting_id: i64,
        dir: &std::path::Path,
        stop: &Arc<AtomicBool>,
        source_bundle: Option<String>,
        ignore: &[String],
    ) -> Result<Option<String>> {
        // Window hunt: the call app may not be open yet at meeting start.
        let deadline = std::time::Instant::now() + Duration::from_secs(FIND_WINDOW_MAX_S);
        let window = loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let content = sc::ShareableContent::current()
                .await
                .map_err(|e| anyhow!("shareable content (screen-recording permission?): {e}"))?;
            if let Some(w) = find_window(&content, source_bundle.as_deref(), ignore) {
                break w;
            }
            if std::time::Instant::now() > deadline {
                log_status(dir, "no call-app window found within 15 minutes; skipped");
                eprintln!(
                    "[noted] meeting {meeting_id}: no meeting-app window found; video skipped"
                );
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(POLL_MS * 5)).await;
        };
        let title = window.title().map(|t| t.to_string()).unwrap_or_default();
        log_status(dir, &format!("recording window {title:?}"));
        println!("[noted] meeting {meeting_id}: recording window “{title}”");

        std::fs::create_dir_all(dir)?;
        let path = dir.join("window.mp4");
        let _ = std::fs::remove_file(&path); // a re-record replaces

        let filter = sc::ContentFilter::with_desktop_independent_window(&window);
        let mut cfg = sc::StreamCfg::new();
        let f = window.frame();
        // Retina-scale the window rect, capped to MAX_EDGE on the long side.
        let (mut w, mut h) = (
            (f.size.width * 2.0) as usize,
            (f.size.height * 2.0) as usize,
        );
        let long = w.max(h);
        if long > MAX_EDGE {
            w = w * MAX_EDGE / long;
            h = h * MAX_EDGE / long;
        }
        cfg.set_width(w.max(2) & !1);
        cfg.set_height(h.max(2) & !1);
        cfg.set_minimum_frame_interval(cm::Time::new(1, FPS));
        cfg.set_shows_cursor(false);

        let mut rec_cfg = sc::RecordingOutputCfg::new();
        rec_cfg.set_output_url(&ns::Url::with_fs_path_str(&path.to_string_lossy(), false));
        let delegate = RecDelegate::with(0);
        let rec_out = sc::RecordingOutput::with_cfg(&rec_cfg, delegate.as_ref());

        let mut stream = sc::Stream::new(&filter, &cfg);
        stream
            .add_recording_output(&rec_out)
            .map_err(|e| anyhow!("add recording output: {e:?}"))?;
        stream
            .start()
            .await
            .map_err(|e| anyhow!("stream start: {e:?}"))?;

        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
        }

        let _ = stream.stop().await;
        // Give the mov writer a beat to finalize the container.
        tokio::time::sleep(Duration::from_millis(400)).await;

        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) == 0 {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        Ok(Some(path.to_string_lossy().to_string()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Smoke test against the live ScreenCaptureKit stack: registers the
        /// delegate class, records ~4s of the largest on-screen window, and
        /// checks a non-empty mp4 lands. Needs Screen Recording permission
        /// for the invoking terminal and macOS 15+.
        ///   OUT=/tmp/smoke.mp4 cargo test video_smoke -- --ignored --nocapture
        #[test]
        #[ignore]
        fn video_smoke() {
            assert!(super::super::video_supported(), "needs macOS 15+");
            // A bare test binary has no window-server connection (CGS_REQUIRE_INIT
            // abort inside SCStream); the shared NSApplication establishes it.
            // The real app is a GUI process, so production never needs this.
            let _app = ns::App::shared();
            let out = std::env::var("OUT").unwrap_or_else(|_| "/tmp/noted-video-smoke.mp4".into());
            let _ = std::fs::remove_file(&out);
            tauri::async_runtime::block_on(async {
                let content = sc::ShareableContent::current()
                    .await
                    .expect("shareable content — grant Screen Recording to the terminal and retry");
                let windows = content.windows();
                let mut best: Option<(f64, cidre::arc::R<sc::Window>)> = None;
                for i in 0..windows.len() {
                    let w = &windows[i];
                    if !w.is_on_screen() || w.window_layer() != 0 {
                        continue;
                    }
                    let f = w.frame();
                    let area = f.size.width * f.size.height;
                    if best.as_ref().map_or(true, |(a, _)| area > *a) {
                        best = Some((area, w.retained()));
                    }
                }
                let (_, window) = best.expect("no on-screen window to record");
                println!(
                    "recording “{}”",
                    window.title().map(|t| t.to_string()).unwrap_or_default()
                );
                let filter = sc::ContentFilter::with_desktop_independent_window(&window);
                let mut cfg = sc::StreamCfg::new();
                let f = window.frame();
                cfg.set_width(((f.size.width as usize * 2).max(2)) & !1);
                cfg.set_height(((f.size.height as usize * 2).max(2)) & !1);
                cfg.set_minimum_frame_interval(cm::Time::new(1, FPS));
                let mut rec_cfg = sc::RecordingOutputCfg::new();
                rec_cfg.set_output_url(&ns::Url::with_fs_path_str(&out, false));
                let delegate = RecDelegate::with(0);
                let rec_out = sc::RecordingOutput::with_cfg(&rec_cfg, delegate.as_ref());
                let mut stream = sc::Stream::new(&filter, &cfg);
                stream.add_recording_output(&rec_out).expect("add output");
                stream.start().await.expect("start");
                tokio::time::sleep(Duration::from_secs(4)).await;
                stream.stop().await.expect("stop");
                tokio::time::sleep(Duration::from_millis(500)).await;
            });
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            println!("recorded {size} bytes to {out}");
            assert!(size > 10_000, "mp4 should be non-trivial, got {size} bytes");
        }
    }
}
