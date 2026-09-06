// Dormant legacy phone bridge. Application startup deliberately cannot bind
// this server while the native iPhone companion is being built. The retained
// implementation exists only to support a bounded migration/audit and must not
// be treated as a product command surface. Its historical roles were:
//   GET  /            -> the FULL noted web app (same UI as desktop), served
//                        from the app's bundled assets (or proxied from the
//                        Vite dev server during `tauri dev`).
//   POST /api/<cmd>   -> RPC bridge: every Tauri command, callable over HTTP so
//                        the web client behaves exactly like the desktop one.
//   GET  /capture     -> the original lightweight photo-only upload page.
//   POST /upload      -> save a photo into the inbox (the /capture page uses it).
//
// It runs over HTTPS (self-signed cert): mobile browsers only grant microphone
// and camera access in a "secure context", so voice capture needs TLS. Every
// /api and /upload call was gated by a random token shown in the desktop app.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tiny_http::Method;

const MAX_LEGACY_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
const MAX_LEGACY_API_BODY_BYTES: usize = 1024 * 1024;

/// Connection info surfaced to the UI (urls contain the token).
pub struct PhoneState {
    /// Primary URL — prefers the stable `<host>.local` name (survives IP changes).
    pub url: String,
    /// Raw-IP fallback URL, for networks where `.local` (mDNS) doesn't resolve.
    pub lan_url: String,
    pub token: String,
    pub port: u16,
}

/// Persisted access token so the phone URL stays stable across launches — a
/// saved "Add to Home Screen" icon keeps working instead of breaking each run.
pub fn load_or_make_token(dir: &Path) -> String {
    let path = dir.join("phone_token.txt");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let t = format!("{:016x}", rand::random::<u64>());
    let _ = std::fs::write(&path, &t);
    t
}

/// The Mac's Bonjour/mDNS name (e.g. "Edisons-MacBook-Pro") for a stable
/// `<name>.local` URL that survives DHCP IP changes. None if unavailable.
pub fn local_hostname() -> Option<String> {
    let out = std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

// ── TLS: a persisted self-signed cert for the current LAN IP ────────────────
// Cached under app_data/tls so the user only accepts the browser warning once.
// Regenerated if the machine's IP changed (the cert's SAN must match the host).
fn load_or_make_cert(dir: &Path, sans: &[String]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let tls = dir.join("tls");
    let _ = std::fs::create_dir_all(&tls);
    let cert_path = tls.join("cert.pem");
    let key_path = tls.join("key.pem");
    let sans_path = tls.join("sans.txt");

    // Reuse the cached cert while the SAN set (hostname + IP) is unchanged, so
    // the user only accepts the browser warning once per host/IP combination.
    let key = sans.join(",");
    let cached = std::fs::read_to_string(&sans_path).unwrap_or_default();
    if cached.trim() == key {
        if let (Ok(c), Ok(k)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            if !c.is_empty() && !k.is_empty() {
                return Ok((c, k));
            }
        }
    }

    let certified = rcgen::generate_simple_self_signed(sans.to_vec())
        .map_err(|e| format!("cert generation failed: {e}"))?;
    let cert_pem = certified.cert.pem().into_bytes();
    let key_pem = certified.key_pair.serialize_pem().into_bytes();
    let _ = std::fs::write(&cert_path, &cert_pem);
    let _ = std::fs::write(&key_path, &key_pem);
    let _ = std::fs::write(&sans_path, &key);
    Ok((cert_pem, key_pem))
}

/// Bind a TCP listener with SO_REUSEADDR set. REUSEADDR (not REUSEPORT) lets us
/// rebind the port while a just-exited process's socket lingers in TIME_WAIT,
/// but the kernel still rejects a *second live* LISTEN — so two overlapping dev
/// instances can't silently split traffic on the same port.
fn try_bind_reuse(port: u16) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    sock.set_reuse_address(true)?;
    let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
    sock.bind(&addr.into())?;
    sock.listen(128)?;
    Ok(sock.into())
}

/// Bind the phone HTTPS server, keeping the port STABLE at `preferred` (8787) so
/// a phone's saved "Add to Home Screen" icon — which bakes in a fixed host:port
/// — keeps working across `tauri dev` restarts. We retry `preferred` for a few
/// seconds to ride out the window where the previous dev process is still
/// exiting, and only drift to an alternate port as a loud last resort.
/// `sans` are the cert's subject-alt-names (hostname.local, LAN IP, localhost).
pub fn bind_https(dir: &Path, sans: &[String], preferred: u16) -> Option<(tiny_http::Server, u16)> {
    let (certificate, private_key) = load_or_make_cert(dir, sans)
        .map_err(|e| eprintln!("[noted] {e}"))
        .ok()?;
    let ssl = || tiny_http::SslConfig {
        certificate: certificate.clone(),
        private_key: private_key.clone(),
    };

    // Hold the preferred port: ~8 tries over ~4s covers a previous instance
    // still releasing the socket during a hot restart.
    for attempt in 0..8 {
        match try_bind_reuse(preferred) {
            Ok(listener) => match tiny_http::Server::from_listener(listener, Some(ssl())) {
                Ok(s) => return Some((s, preferred)),
                Err(e) => eprintln!("[noted] phone TLS setup on :{preferred} failed: {e}"),
            },
            Err(_) if attempt < 7 => std::thread::sleep(std::time::Duration::from_millis(500)),
            Err(_) => {}
        }
    }

    // Last resort: drift to an adjacent port, but warn loudly — a saved phone
    // icon points at `preferred` and won't reach this one.
    for port in [preferred + 1, preferred + 2] {
        if let Ok(listener) = try_bind_reuse(port) {
            if let Ok(s) = tiny_http::Server::from_listener(listener, Some(ssl())) {
                eprintln!(
                    "[noted] WARNING: phone port {preferred} was held, bound :{port} instead. \
                     Your saved phone icon expects :{preferred}; free it with \
                     `lsof -nP -iTCP:{preferred} -sTCP:LISTEN` and relaunch."
                );
                return Some((s, port));
            }
        }
    }
    None
}

/// Spawn a small pool of worker threads that each handle requests. A handful is
/// plenty for one person's phone + desktop and lets the initial parallel loads
/// (notes, categories, health) overlap instead of serializing.
pub fn serve(server: tiny_http::Server, app: AppHandle, inbox: PathBuf, token: String) {
    let server = std::sync::Arc::new(server);
    for _ in 0..4 {
        let server = server.clone();
        let app = app.clone();
        let inbox = inbox.clone();
        let token = token.clone();
        std::thread::spawn(move || {
            while let Ok(req) = server.recv() {
                handle_request(&app, &inbox, &token, req);
            }
        });
    }
}

fn handle_request(app: &AppHandle, inbox: &Path, token: &str, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("/").to_string();
    let method = req.method().clone();

    // POST /upload — the lightweight photo capture path (unchanged behavior).
    if method == Method::Post && path == "/upload" {
        if !query_token_ok(&url, token) {
            let _ =
                req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
            return;
        }
        let ext = content_type_ext(&req);
        let bytes = match read_body_limited(req.as_reader(), MAX_LEGACY_UPLOAD_BYTES) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) | Err(BodyReadError::Io) => {
                let _ = req.respond(tiny_http::Response::from_string("bad").with_status_code(400));
                return;
            }
            Err(BodyReadError::TooLarge) => {
                let _ = req.respond(
                    tiny_http::Response::from_string("payload too large").with_status_code(413),
                );
                return;
            }
        };
        match save_and_notify(app, inbox, &bytes, &ext) {
            Ok(_) => {
                let _ = req.respond(tiny_http::Response::from_string("ok"));
            }
            Err(e) => {
                let _ = req.respond(tiny_http::Response::from_string(e).with_status_code(500));
            }
        }
        return;
    }

    // POST /api/<command> — retained diagnostic route. The allowlist below is
    // the effective boundary; the historical dispatcher is not product API.
    if method == Method::Post && path.starts_with("/api/") {
        if !query_token_ok(&url, token) {
            let _ =
                req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
            return;
        }
        let cmd = path.trim_start_matches("/api/").to_string();
        if !legacy_phone_command_is_allowed(&cmd) {
            let _ = req.respond(
                tiny_http::Response::from_string("legacy phone command disabled")
                    .with_status_code(410),
            );
            return;
        }
        let body = match read_body_limited(req.as_reader(), MAX_LEGACY_API_BODY_BYTES) {
            Ok(body) => body,
            Err(BodyReadError::TooLarge) => {
                let _ = req.respond(
                    tiny_http::Response::from_string("payload too large").with_status_code(413),
                );
                return;
            }
            Err(BodyReadError::Io) => {
                let _ = req.respond(tiny_http::Response::from_string("bad").with_status_code(400));
                return;
            }
        };
        let args: Value = match serde_json::from_slice(&body) {
            Ok(args) => args,
            Err(_) => {
                let _ = req.respond(
                    tiny_http::Response::from_string("invalid json").with_status_code(400),
                );
                return;
            }
        };
        match tauri::async_runtime::block_on(handle_api(app, &cmd, &args)) {
            Ok(v) => {
                let _ = req.respond(json_response(&v));
            }
            Err(e) => {
                let _ = req.respond(tiny_http::Response::from_string(e).with_status_code(500));
            }
        }
        return;
    }

    // Everything else: serve the web app shell (or the capture page).
    serve_static(app, &path, req);
}

#[derive(Debug, Eq, PartialEq)]
enum BodyReadError {
    Io,
    TooLarge,
}

fn read_body_limited(
    reader: &mut dyn std::io::Read,
    limit: usize,
) -> Result<Vec<u8>, BodyReadError> {
    let mut bytes = Vec::new();
    let mut limited = std::io::Read::take(reader, (limit as u64).saturating_add(1));
    std::io::Read::read_to_end(&mut limited, &mut bytes).map_err(|_| BodyReadError::Io)?;
    if bytes.len() > limit {
        return Err(BodyReadError::TooLarge);
    }
    Ok(bytes)
}

/// The retained bridge is diagnostic-only. Keep the allowlist explicit and
/// small instead of comparing it with the desktop command registry.
fn legacy_phone_command_is_allowed(command: &str) -> bool {
    matches!(command, "health")
}

/// Historical dispatcher retained temporarily for migration auditing. Its caller
/// must enforce `legacy_phone_command_is_allowed`; the native companion will use
/// a separate typed sync/job boundary rather than this desktop command mirror.
async fn handle_api(app: &AppHandle, cmd: &str, b: &Value) -> Result<Value, String> {
    let a = app.clone();
    match cmd {
        "team_notification_send" | "team_notification_take_target" => Err("Desktop-only command".into()),
        "theme_state" => crate::theme_state(a).await,
        "system_settings_get" => crate::system_settings_get()
            .await
            .and_then(|settings| serde_json::to_value(settings).map_err(|e| e.to_string())),
        "system_settings_set" => crate::system_settings_set(
            a,
            sarg(b, "timeZone"),
            b.get("preferredName")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
            .await
            .and_then(|settings| serde_json::to_value(settings).map_err(|e| e.to_string())),
        "theme_list" => crate::theme_list(a).await,
        "theme_save" => {
            let pack: crate::themes::ThemePack =
                serde_json::from_value(varg(b, "pack")).map_err(|e| e.to_string())?;
            crate::theme_save(a, pack).await
        }
        "theme_activate" => {
            crate::theme_activate(a, sarg(b, "themeId"), oarg(b, "colorMode")).await
        }
        "theme_set_color_mode" => crate::theme_set_color_mode(a, sarg(b, "colorMode")).await,
        "theme_delete" => crate::theme_delete(a, sarg(b, "themeId")).await,
        "theme_compile_design" => {
            crate::theme_compile_design(sarg(b, "designMd"), oarg(b, "name")).await
        }
        "theme_suggest" => {
            let candidates: Vec<crate::themes::ThemeCandidate> =
                serde_json::from_value(varg(b, "candidates")).map_err(|e| e.to_string())?;
            crate::theme_suggest(sarg(b, "prompt"), candidates).await
        }
        "health" => crate::health(a).await,
        "categorize_note" => crate::categorize_note(a, sarg(b, "text")).await,
        "ocr_photo" => crate::ocr_photo(sarg(b, "imageBase64"))
            .await
            .map(|s| json!(s)),
        "categorize_photo" => crate::categorize_photo(a, sarg(b, "imageBase64")).await,
        "save_image" => crate::save_image(a, sarg(b, "imageBase64"), sarg(b, "ext"))
            .await
            .map(|s| json!(s)),
        "load_image" => crate::load_image(a, sarg(b, "path"))
            .await
            .map(|image| json!(image)),
        "save_entry" => {
            let args: crate::SaveArgs =
                serde_json::from_value(varg(b, "args")).map_err(|e| e.to_string())?;
            crate::save_entry(a, args).await.map(|n| json!(n))
        }
        "quick_capture" => crate::quick_capture(
            a,
            sarg(b, "rawText"),
            oarg(b, "source"),
            oarg(b, "imagePath"),
            oarg(b, "eventDate"),
            oarg(b, "filingContext"),
        )
        .await
        .map(|id| json!(id)),
        "list_notes" => crate::list_notes(a).await,
        "create_note_document" => {
            let folder_id = b.get("folderId").and_then(|v| v.as_i64());
            crate::create_note_document(
                a,
                sarg(b, "title"),
                sarg(b, "rawText"),
                sarg(b, "documentJson"),
                sarg(b, "filingContext"),
                folder_id,
            )
            .await
            .map(|id| json!(id))
        }
        "note_trash_list" => crate::note_trash_list(a).await,
        "note_trash" => crate::note_trash(a, iarg(b, "noteId"))
            .await
            .map(|_| Value::Null),
        "note_restore" => crate::note_restore(a, iarg(b, "noteId"))
            .await
            .map(|_| Value::Null),
        "note_delete_forever" => crate::note_delete_forever(a, iarg(b, "noteId"))
            .await
            .map(|_| Value::Null),
        "update_note" => {
            crate::update_note(
                a,
                iarg(b, "noteId"),
                sarg(b, "title"),
                sarg(b, "rawText"),
                oarg(b, "documentJson"),
            )
                .await
                .map(|_| Value::Null)
        }
        "list_categories" => crate::list_categories(a).await,
        "list_note_folders" => crate::list_note_folders(a).await,
        "create_note_folder" => {
            let parent_id = b.get("parentId").and_then(|v| v.as_i64());
            crate::create_note_folder(a, parent_id, sarg(b, "name"), sarg(b, "kind"))
                .await
                .map(|id| json!(id))
        }
        "rename_note_folder" => crate::rename_note_folder(a, iarg(b, "folderId"), sarg(b, "name"))
            .await
            .map(|_| Value::Null),
        "move_note_folder" => {
            let parent_id = b.get("parentId").and_then(|v| v.as_i64());
            let before_id = b.get("beforeId").and_then(|v| v.as_i64());
            crate::move_note_folder(a, iarg(b, "folderId"), parent_id, before_id)
                .await
                .map(|_| Value::Null)
        }
        "delete_note_folder" => crate::delete_note_folder(a, iarg(b, "folderId"))
            .await
            .map(|_| Value::Null),
        "file_note" => {
            let folder_id = b.get("folderId").and_then(|v| v.as_i64());
            crate::file_note(a, iarg(b, "noteId"), folder_id)
                .await
                .map(|receipt| json!(receipt))
        }
        "undo_note_filing" => crate::undo_note_filing(a, iarg(b, "eventId"))
            .await
            .map(|receipt| json!(receipt)),
        "chat" => {
            let history: Vec<crate::ChatMsg> =
                serde_json::from_value(varg(b, "history")).unwrap_or_default();
            let entity_id = b.get("entityId").and_then(|v| v.as_i64());
            crate::chat(a, sarg(b, "question"), history, oarg(b, "scope"), entity_id).await
        }
        "reminder_settings_get" => {
            serde_json::to_value(crate::reminders::get()).map_err(|error| error.to_string())
        }
        "reminder_settings_set" => {
            let settings =
                serde_json::from_value(varg(b, "settings")).map_err(|error| error.to_string())?;
            let dir = a.path().app_data_dir().map_err(|error| error.to_string())?;
            crate::reminders::update(&dir, settings)
                .and_then(|value| serde_json::to_value(value).map_err(Into::into))
                .map_err(|error| error.to_string())
        }
        "create_category" => crate::create_category(a, sarg(b, "name"), sarg(b, "description"))
            .await
            .map(|n| json!(n)),
        "update_entry" => crate::update_entry(a, iarg(b, "entryId"), varg(b, "data"))
            .await
            .map(|n| json!(n)),
        "speak" => crate::speak(sarg(b, "text")).map(|_| Value::Null),
        "stop_speaking" => {
            crate::stop_speaking();
            Ok(Value::Null)
        }
        "reindex" => crate::reindex(a).await.map(|n| json!(n)),
        "backfill_entities" => crate::backfill_entities(a).await.map(|n| json!(n)),
        "category_trends" => crate::category_trends(a, sarg(b, "category")).await,
        "generate_recap" => crate::generate_recap(a, sarg(b, "period")).await,
        "backfill_recaps" => crate::backfill_recaps(a).await.map(|_| Value::Null),
        "list_recaps" => crate::list_recaps(a).await,
        "export_db" => Err("backups can only be created from the desktop app".into()),
        "phone_info" => Ok(crate::phone_info(a)),
        "read_inbox_image" => crate::read_inbox_image(a, sarg(b, "path")).await,
        "voice_status" => Ok(crate::voice_status(a)),
        "download_voice_model" => crate::download_voice_model(a).await.map(|ok| json!(ok)),
        "transcribe" => crate::transcribe(a, sarg(b, "audioB64"), iarg(b, "sampleRate") as u32)
            .await
            .map(|s| json!(s)),
        // Meetings: reads + notes work from the phone; capture/summarize need
        // the desktop's audio devices and model — clean error, never a 404.
        "team_status" => crate::team_status(a),
        "team_request" => crate::team_request(a, sarg(b, "method"), sarg(b, "path"), b.get("body").cloned()).await,
        "team_ask" => crate::team_ask(a, sarg(b, "org"), varg(b, "body")).await,
        "team_save_attachment" => Err("Save team attachments from the desktop app".into()),
        "team_connect" | "team_disconnect" | "team_publish_meeting" => Err("Manage team connections and publish local meetings from the desktop app".into()),
        "meeting_model_status" => Ok(crate::meeting_model_status(a)),
        "meeting_state" => Ok(crate::meeting_state(a)),
        "meeting_list" => crate::meeting_list(a).await,
        "meeting_filing_rules" => crate::meeting_filing_rules(a).await,
        "meeting_filing_rule_set" => {
            let priority = b.get("priority").and_then(Value::as_i64);
            crate::meeting_filing_rule_set(a, sarg(b, "email"), iarg(b, "folderId"), priority).await
        }
        "meeting_filing_rule_delete" => crate::meeting_filing_rule_delete(a, sarg(b, "email"))
            .await
            .map(|deleted| json!(deleted)),
        "meeting_filing_rules_reorder" => {
            let emails: Vec<String> =
                serde_json::from_value(varg(b, "emails")).map_err(|e| e.to_string())?;
            crate::meeting_filing_rules_reorder(a, emails).await
        }
        "meeting_filing_backfill_preview" => crate::meeting_filing_backfill_preview(a).await,
        "meeting_filing_backfill_apply" => {
            crate::meeting_filing_backfill_apply(a, sarg(b, "token")).await
        }
        "meeting_search_transcripts" => {
            let filters = b
                .get("filters")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok());
            let sort = b.get("sort").and_then(Value::as_str).map(str::to_owned);
            crate::meeting_search_transcripts(a, sarg(b, "query"), iarg(b, "limit"), filters, sort)
                .await
        }
        "meeting_search_facets" => crate::meeting_search_facets(a).await,
        "meeting_transcript_vocabulary_list" => crate::meeting_transcript_vocabulary_list(a).await,
        "meeting_transcript_vocabulary_preview" => {
            crate::meeting_transcript_vocabulary_preview(a, sarg(b, "heard")).await
        }
        "meeting_transcript_vocabulary_apply" => {
            crate::meeting_transcript_vocabulary_apply(a, sarg(b, "heard"), sarg(b, "preferred"))
                .await
        }
        "meeting_transcript_vocabulary_remove" => {
            crate::meeting_transcript_vocabulary_remove(a, iarg(b, "id"))
                .await
                .map(|_| Value::Null)
        }
        "meeting_transcript_vocabulary_undo" => {
            crate::meeting_transcript_vocabulary_undo(a, iarg(b, "batchId")).await
        }
        "meeting_trash_list" => crate::meeting_trash_list(a).await,
        "meeting_get" => crate::meeting_get(a, iarg(b, "id")).await,
        "meeting_trash" => crate::meeting_trash(a, iarg(b, "id"))
            .await
            .map(|_| Value::Null),
        "meeting_restore" => crate::meeting_restore(a, iarg(b, "id"))
            .await
            .map(|_| Value::Null),
        "meeting_delete_forever" => crate::meeting_delete_forever(a, iarg(b, "id"))
            .await
            .map(|_| Value::Null),
        "meeting_set_notes" => crate::meeting_set_notes(
            a,
            iarg(b, "id"),
            sarg(b, "notes"),
            oarg(b, "notesDocumentJson"),
        )
        .await
        .map(|_| Value::Null),
        "meeting_set_title" => crate::meeting_set_title(a, iarg(b, "id"), sarg(b, "title"))
            .await
            .map(|_| Value::Null),
        "meeting_set_filing_destination" => {
            crate::meeting_set_filing_destination(a, iarg(b, "id"), iarg(b, "folderId"))
                .await
                .map(|_| Value::Null)
        }
        "meeting_set_summary" => {
            crate::meeting_set_summary(a, iarg(b, "id"), iarg(b, "summaryId"), sarg(b, "contentMd"))
                .await
                .map(|_| Value::Null)
        }
        "meeting_templates" => crate::meeting_templates(a).await,
        "meeting_rename_speaker" => {
            crate::meeting_rename_speaker(a, iarg(b, "id"), sarg(b, "from"), sarg(b, "to"))
                .await
                .map(|_| Value::Null)
        }
        "meeting_rename_speakers" => {
            let changes = serde_json::from_value(varg(b, "changes")).map_err(|e| e.to_string())?;
            crate::meeting_rename_speakers(a, iarg(b, "id"), changes)
                .await
                .map(|result| json!(result))
        }
        // Rediarize runs on the desktop's model but touches no capture
        // hardware — safe to trigger remotely, like re-summarize.
        "meeting_rediarize" => crate::meeting_rediarize(a, iarg(b, "id"))
            .await
            .map(|result| json!(result)),
        "meeting_video_delete" => crate::meeting_video_delete(a, iarg(b, "id"))
            .await
            .map(|_| Value::Null),
        // Assist answers run on the desktop's model; asking from the phone is
        // a remote control, like summarize.
        "meeting_assist" => crate::meeting_assist(a, iarg(b, "id"), sarg(b, "question")).await,
        "meetings_settings_get" => Ok(crate::meetings_settings_get()),
        // Stopping works from the phone too — it's a remote control for the
        // desktop recorder (capture itself always runs on the Mac).
        "meeting_stop" => crate::meeting_stop(a).await.map(|v| json!(v)),
        // Summarize also works remotely — the model runs on the Mac either way,
        // and re-summarizing a finished meeting touches no capture hardware.
        "meeting_summarize" => crate::meeting_summarize(a, iarg(b, "id"), oarg(b, "template"))
            .await
            .map(|v| json!(v)),
        "meeting_start"
        | "agent_access_status"
        | "agent_access_set_enabled"
        | "agent_client_create"
        | "agent_client_revoke"
        | "agent_context_pending"
        | "agent_context_preview"
        | "agent_context_resolve"
        | "agent_context_receipts"
        | "meeting_template_save"
        | "meeting_video_request_permission"
        | "meeting_template_delete"
        | "meeting_capture_probe"
        | "download_meeting_model"
        | "download_speaker_model"
        | "download_in_person_diarizer"
        | "download_parakeet_model"
        | "meeting_prompt_payload"
        | "meeting_dismiss_prompt"
        | "meetings_settings_set"
        | "hosted_key_set"
        | "meeting_export_md"
        | "meeting_export_pdf"
        | "set_chrome_theme" => Err("this action runs on the desktop app only".into()),
        "list_entities" => crate::list_entities(a).await,
        "merge_entities" => crate::merge_entities(a, iarg(b, "keep"), iarg(b, "drop"))
            .await
            .map(|_| Value::Null),
        "suggest_entity_merges" => crate::suggest_entity_merges(a).await,
        "dismiss_merge_suggestion" => {
            crate::dismiss_merge_suggestion(a, iarg(b, "a"), iarg(b, "b"))
                .await
                .map(|_| Value::Null)
        }
        "entity_graph" => crate::entity_graph(a).await,
        "entity_detail" => crate::entity_detail(a, iarg(b, "entityId")).await,
        "entity_profile" => crate::entity_profile(a, iarg(b, "entityId")).await,
        "list_people" => crate::list_people(a).await,
        "suggest_person_names" => crate::suggest_person_names(a).await,
        "confirm_person_name" => {
            crate::confirm_person_name(a, iarg(b, "entityId"), sarg(b, "name"))
                .await
                .map(|_| Value::Null)
        }
        "dismiss_person_name" => crate::dismiss_person_name(a, iarg(b, "entityId"))
            .await
            .map(|_| Value::Null),
        "kg_reindex_meetings" => crate::kg_reindex_meetings(a).await,
        "get_provider_settings" => Ok(crate::get_provider_settings()),
        // Keys are camelCase — they must match what api.ts sends (the old
        // snake_case reads silently dropped every optional arg to None).
        "set_provider_settings" => crate::set_provider_settings(
            a,
            sarg(b, "mode"),
            b.get("confirmEmbeddingRebuild")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            oarg(b, "cloudProvider"),
            oarg(b, "geminiApiKey"),
            oarg(b, "geminiTextModel"),
            oarg(b, "geminiVisionModel"),
            oarg(b, "openaiBaseUrl"),
            oarg(b, "openaiApiKey"),
            oarg(b, "openaiTextModel"),
            oarg(b, "openaiVisionModel"),
            oarg(b, "anthropicApiKey"),
            oarg(b, "anthropicTextModel"),
            oarg(b, "anthropicVisionModel"),
            oarg(b, "textModel"),
            oarg(b, "visionModel"),
        )
        .map(|_| Value::Null),
        "test_provider" => crate::test_provider().await.map(|s| json!(s)),
        "gcal_auth_status" => Ok(crate::gcal_auth_status()),
        "gcal_set_client" => {
            crate::gcal_set_client(a, sarg(b, "clientId"), sarg(b, "clientSecret"))
                .map(|_| Value::Null)
        }
        "gcal_begin_auth" => crate::gcal_begin_auth(a).await,
        "gcal_disconnect" => crate::gcal_disconnect(a).map(|_| Value::Null),
        "gcal_sync" => crate::gcal_sync(a, oarg(b, "eventDate")).await,
        "gcal_clear_day" => crate::gcal_clear_day(a, oarg(b, "eventDate"))
            .await
            .map(|n| json!(n)),
        "gcal_list_events" => crate::gcal_list_events(a, oarg(b, "eventDate")).await,
        "gcal_remove_account" => crate::gcal_remove_account(a, sarg(b, "email")),
        "gcal_set_calendar_enabled" => crate::gcal_set_calendar_enabled(
            a,
            sarg(b, "account"),
            sarg(b, "calendarId"),
            b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        ),
        "applecal_status" => crate::applecal_status(),
        "applecal_request_access" => crate::applecal_request_access(a).await,
        "applecal_set_calendar_enabled" => crate::applecal_set_calendar_enabled(
            a,
            sarg(b, "calendarId"),
            b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        ),
        "set_byok_settings" => Err("this action runs on the desktop app only".into()),
        "list_byok_models" => Err("this action runs on the desktop app only".into()),
        "test_byok_settings" => Err("this action runs on the desktop app only".into()),
        "gcal_refresh_calendars" => crate::gcal_refresh_calendars(a).await,
        "gcal_set_sync_account" => crate::gcal_set_sync_account(a, sarg(b, "email")),
        "gcal_contacts" => Ok(crate::gcal_contacts()),
        "gcal_events_range" => {
            crate::gcal_events_range(a, sarg(b, "startDate"), sarg(b, "endDate")).await
        }
        "gcal_create_event" => {
            crate::gcal_create_event(
                a,
                sarg(b, "account"),
                sarg(b, "calendarId"),
                sarg(b, "title"),
                sarg(b, "date"),
                oarg(b, "start"),
                oarg(b, "end"),
                oarg(b, "endDate"),
                oarg(b, "location"),
                oarg(b, "description"),
                b.get("addMeet").and_then(|v| v.as_bool()),
                b.get("guests")
                    .and_then(|v| serde_json::from_value(v.clone()).ok()),
            )
            .await
        }
        "gcal_update_event" => crate::gcal_update_event(
            a,
            sarg(b, "account"),
            sarg(b, "calendarId"),
            sarg(b, "eventId"),
            sarg(b, "title"),
            sarg(b, "date"),
            oarg(b, "start"),
            oarg(b, "end"),
            oarg(b, "endDate"),
            oarg(b, "location"),
            oarg(b, "description"),
            oarg(b, "moveTo"),
            b.get("meet").and_then(|v| v.as_bool()),
        )
        .await
        .map(|_| Value::Null),
        "gcal_delete_event" => crate::gcal_delete_event(
            a,
            sarg(b, "account"),
            sarg(b, "calendarId"),
            sarg(b, "eventId"),
        )
        .await
        .map(|_| Value::Null),
        "journal_reflect" => {
            let history: Vec<crate::ChatMsg> =
                serde_json::from_value(varg(b, "history")).unwrap_or_default();
            crate::journal_reflect(a, sarg(b, "text"), history).await
        }
        "brain_list_vaults" => crate::brain_list_vaults(a).await,
        "brain_add_vault" => crate::brain_add_vault(a, sarg(b, "path"), oarg(b, "direction")).await,
        "brain_remove_vault" => crate::brain_remove_vault(a, sarg(b, "vault"))
            .await
            .map(|_| Value::Null),
        "brain_sync" => crate::brain_sync(a, oarg(b, "vault")).await,
        "work_graph" => crate::work_graph(a, oarg(b, "vault")).await,
        "brain_write_preview" => crate::brain_write_preview(a, oarg(b, "vault")).await,
        "brain_write_back" => crate::brain_write_back(a, oarg(b, "vault")).await,
        "personal_export_preview" => crate::personal_export_preview(a).await,
        "personal_export" => crate::personal_export(a).await,
        "related_brain" => crate::related_brain(a, sarg(b, "text")).await,
        "brain_get_auto" => Ok(json!(crate::brain_get_auto())),
        "brain_set_auto" => crate::brain_set_auto(
            a,
            b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        )
        .map(|_| Value::Null),
        other => Err(format!("unknown command: {other}")),
    }
}

// ── arg helpers (read frontend-shaped JSON keys) ────────────────────────────
fn sarg(b: &Value, k: &str) -> String {
    b.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}
fn iarg(b: &Value, k: &str) -> i64 {
    b.get(k).and_then(|v| v.as_i64()).unwrap_or(0)
}
fn varg(b: &Value, k: &str) -> Value {
    b.get(k).cloned().unwrap_or(Value::Null)
}
fn oarg(b: &Value, k: &str) -> Option<String> {
    b.get(k).and_then(|v| v.as_str()).map(String::from)
}

// ── static assets: bundled SPA in release, Vite proxy in dev ────────────────
fn serve_static(app: &AppHandle, path: &str, req: tiny_http::Request) {
    // The public manifest must never contain a bearer token. The legacy bridge
    // is not an installable phone product, and authentication material does not
    // belong in URLs, browser history, logs, or unauthenticated static routes.
    if path == "/manifest.webmanifest" {
        let _ = req.respond(
            tiny_http::Response::from_string(manifest_json())
                .with_header(header("Content-Type", "application/manifest+json")),
        );
        return;
    }
    if path == "/capture" {
        let _ = req.respond(html_response(PAGE));
        return;
    }
    let rel = if path == "/" || path.is_empty() {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };

    // 1) Bundled assets (release / `tauri build`).
    let resolver = app.asset_resolver();
    if let Some(asset) = resolver
        .get(format!("/{rel}"))
        .or_else(|| resolver.get(rel.to_string()))
    {
        // Tauri's mime guesser doesn't know .webmanifest — correct it so the
        // browser recognizes the PWA manifest (otherwise install is flaky).
        let ct = mime_override(rel).unwrap_or(asset.mime_type.as_str());
        let _ = req.respond(
            tiny_http::Response::from_data(asset.bytes).with_header(header("Content-Type", ct)),
        );
        return;
    }

    // 2) Historical dev fallback. Application startup no longer exposes it.
    let target = format!("http://localhost:1420/{rel}");
    let fetched = tauri::async_runtime::block_on(async {
        let r = reqwest::get(&target).await.ok()?;
        let ct = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = r.bytes().await.ok()?;
        Some((ct, bytes.to_vec()))
    });
    match fetched {
        Some((ct, bytes)) => {
            // no-store so a phone in dev never serves a stale module (release
            // assets are content-hashed, so this path is dev-only anyway).
            let _ = req.respond(
                tiny_http::Response::from_data(bytes)
                    .with_header(header("Content-Type", &ct))
                    .with_header(header("Cache-Control", "no-store")),
            );
        }
        None => {
            let _ =
                req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
    }
}

fn manifest_json() -> String {
    json!({
        "name": "noted",
        "short_name": "noted",
        "description": "Your personal log — capture and review.",
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "background_color": "#0e0f13",
        "theme_color": "#0e0f13",
        "icons": [
            { "src": "/pwa-192.png", "sizes": "192x192", "type": "image/png" },
            { "src": "/pwa-512.png", "sizes": "512x512", "type": "image/png" },
            { "src": "/pwa-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
        ]
    })
    .to_string()
}

// Correct content-types Tauri's guesser gets wrong (notably .webmanifest).
fn mime_override(path: &str) -> Option<&'static str> {
    if path.ends_with(".webmanifest") {
        Some("application/manifest+json")
    } else {
        None
    }
}

// ── small response helpers ──────────────────────────────────────────────────
fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .unwrap_or_else(|_| tiny_http::Header::from_bytes(&b"X-Noted"[..], &b"1"[..]).unwrap())
}

fn json_response(v: &Value) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
    tiny_http::Response::from_string(body).with_header(header("Content-Type", "application/json"))
}

fn save_and_notify(app: &AppHandle, inbox: &Path, bytes: &[u8], ext: &str) -> Result<(), String> {
    // unique-enough filename without Date::now() shenanigans
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let path = inbox.join(format!("{stamp}.{ext}"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    app.emit("photo-received", json!({ "path": path.to_string_lossy() }))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn query_token_ok(url: &str, token: &str) -> bool {
    url.split('?')
        .nth(1)
        .map(|q| q.split('&').any(|kv| kv == format!("t={token}")))
        .unwrap_or(false)
}

fn content_type_ext(req: &tiny_http::Request) -> String {
    let ct = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    if ct.contains("png") {
        "png".into()
    } else if ct.contains("heic") {
        "heic".into()
    } else {
        "jpg".into()
    }
}

fn html_response(body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
            .unwrap();
    tiny_http::Response::from_string(body).with_header(header)
}

const PAGE: &str = r#"<!doctype html><html><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>noted · capture</title>
<style>
  :root{color-scheme:dark}
  body{margin:0;background:#0e0f13;color:#e7e9ee;font-family:-apple-system,system-ui,sans-serif;
       display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh;padding:24px;box-sizing:border-box}
  h1{font-size:26px;margin:0 0 6px}.dot{color:#6ea8fe}
  p{color:#8b90a0;margin:0 0 28px;text-align:center}
  label{display:block;background:linear-gradient(180deg,#6ea8fe,#5a8fe6);color:#0b1020;font-weight:700;
        font-size:18px;padding:18px 26px;border-radius:14px;text-align:center;width:100%;max-width:320px}
  input{display:none}
  #status{margin-top:22px;font-size:15px;min-height:22px}
  .ok{color:#5fd0a0}.err{color:#ff6b6b}.busy{color:#ffb454}
</style></head><body>
<h1>noted<span class="dot">.</span></h1>
<p>Snap a note — it appears on your desktop.</p>
<label for="f">📷 Take / choose photo</label>
<input id="f" type="file" accept="image/*" capture="environment">
<div id="status"></div>
<script>
  const t = new URLSearchParams(location.search).get('t') || '';
  const s = document.getElementById('status');
  document.getElementById('f').addEventListener('change', async (e) => {
    const file = e.target.files[0]; if(!file) return;
    s.className='busy'; s.textContent='Sending…';
    try{
      const r = await fetch('/upload?t='+encodeURIComponent(t), {method:'POST', headers:{'Content-Type':file.type||'image/jpeg'}, body:file});
      if(r.ok){ s.className='ok'; s.textContent='✓ Sent! Review it on your desktop.'; }
      else { s.className='err'; s.textContent='Upload failed ('+r.status+')'; }
    }catch(err){ s.className='err'; s.textContent='Could not reach noted. Same wifi?'; }
    e.target.value='';
  });
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn public_manifest_never_contains_a_session_secret() {
        let secret = "sentinel-phone-session-secret";
        let manifest = manifest_json();
        let parsed: Value = serde_json::from_str(&manifest).unwrap();

        assert_eq!(parsed["start_url"], "/");
        assert!(!manifest.contains(secret));
        assert!(!manifest.contains("?t="));
    }

    #[test]
    fn legacy_api_allows_health_only() {
        assert!(legacy_phone_command_is_allowed("health"));
        for sensitive in [
            "export_db",
            "read_inbox_image",
            "note_delete_forever",
            "meeting_delete_forever",
            "set_provider_settings",
            "gcal_set_client",
            "brain_add_vault",
            "agent_access_set_policy",
        ] {
            assert!(
                !legacy_phone_command_is_allowed(sensitive),
                "sensitive legacy command remained reachable: {sensitive}"
            );
        }
    }

    #[test]
    fn body_reader_accepts_the_limit_and_rejects_one_byte_more() {
        let mut exact = Cursor::new(vec![7_u8; 16]);
        assert_eq!(read_body_limited(&mut exact, 16).unwrap().len(), 16);

        let mut oversized = Cursor::new(vec![7_u8; 17]);
        assert_eq!(
            read_body_limited(&mut oversized, 16),
            Err(BodyReadError::TooLarge)
        );
    }
}
