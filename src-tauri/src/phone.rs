// Phone access: a small LAN server so your phone can use noted while your
// computer does the work. Two roles:
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
// /api and /upload call is gated by a random token shown in the desktop app.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tiny_http::Method;

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
            let _ = req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
            return;
        }
        let ext = content_type_ext(&req);
        let mut bytes = Vec::new();
        if req.as_reader().read_to_end(&mut bytes).is_err() || bytes.is_empty() {
            let _ = req.respond(tiny_http::Response::from_string("bad").with_status_code(400));
            return;
        }
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

    // POST /api/<command> — the full RPC bridge for the web client.
    if method == Method::Post && path.starts_with("/api/") {
        if !query_token_ok(&url, token) {
            let _ = req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
            return;
        }
        let cmd = path.trim_start_matches("/api/").to_string();
        let mut body = Vec::new();
        let _ = req.as_reader().read_to_end(&mut body);
        let args: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
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

/// Dispatch one HTTP RPC to the matching Tauri command. The argument keys mirror
/// exactly what the frontend passes to `invoke(...)`, so the desktop and web
/// transports stay byte-for-byte compatible.
async fn handle_api(app: &AppHandle, cmd: &str, b: &Value) -> Result<Value, String> {
    let a = app.clone();
    match cmd {
        "health" => crate::health(a).await,
        "categorize_note" => crate::categorize_note(a, sarg(b, "text")).await,
        "ocr_photo" => crate::ocr_photo(sarg(b, "imageBase64")).await.map(|s| json!(s)),
        "categorize_photo" => crate::categorize_photo(a, sarg(b, "imageBase64")).await,
        "save_image" => crate::save_image(a, sarg(b, "imageBase64"), sarg(b, "ext")).await.map(|s| json!(s)),
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
        )
        .await
        .map(|id| json!(id)),
        "list_notes" => crate::list_notes(a).await,
        "list_categories" => crate::list_categories(a).await,
        "chat" => {
            let history: Vec<crate::ChatMsg> =
                serde_json::from_value(varg(b, "history")).unwrap_or_default();
            let entity_id = b.get("entityId").and_then(|v| v.as_i64());
            crate::chat(a, sarg(b, "question"), history, oarg(b, "scope"), entity_id).await
        }
        "create_category" => {
            crate::create_category(a, sarg(b, "name"), sarg(b, "description")).await.map(|n| json!(n))
        }
        "update_entry" => {
            crate::update_entry(a, iarg(b, "entryId"), varg(b, "data")).await.map(|n| json!(n))
        }
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
        "export_db" => crate::export_db(a).await.map(|s| json!(s)),
        "phone_info" => Ok(crate::phone_info(a)),
        "read_inbox_image" => crate::read_inbox_image(sarg(b, "path")).await,
        "voice_status" => Ok(crate::voice_status(a)),
        "download_voice_model" => crate::download_voice_model(a).await.map(|ok| json!(ok)),
        "transcribe" => {
            crate::transcribe(a, sarg(b, "audioB64"), iarg(b, "sampleRate") as u32)
                .await
                .map(|s| json!(s))
        }
        // Meetings: reads + notes work from the phone; capture/summarize need
        // the desktop's audio devices and model — clean error, never a 404.
        "meeting_model_status" => Ok(crate::meeting_model_status(a)),
        "meeting_state" => Ok(crate::meeting_state(a)),
        "meeting_list" => crate::meeting_list(a).await,
        "meeting_get" => crate::meeting_get(a, iarg(b, "id")).await,
        "meeting_set_notes" => crate::meeting_set_notes(a, iarg(b, "id"), sarg(b, "notes"))
            .await
            .map(|_| Value::Null),
        "meeting_templates" => crate::meeting_templates(a).await,
        "meetings_settings_get" => Ok(crate::meetings_settings_get()),
        "meeting_start" | "meeting_stop" | "meeting_summarize" | "meeting_template_save"
        | "meeting_template_delete" | "meeting_capture_probe" | "download_meeting_model"
        | "meeting_prompt_payload" | "meeting_dismiss_prompt" | "meetings_settings_set" => {
            Err("this action runs on the desktop app only".into())
        }
        "list_entities" => crate::list_entities(a).await,
        "merge_entities" => {
            crate::merge_entities(a, iarg(b, "keep"), iarg(b, "drop")).await.map(|_| Value::Null)
        }
        "suggest_entity_merges" => crate::suggest_entity_merges(a).await,
        "dismiss_merge_suggestion" => {
            crate::dismiss_merge_suggestion(a, iarg(b, "a"), iarg(b, "b")).await.map(|_| Value::Null)
        }
        "entity_graph" => crate::entity_graph(a).await,
        "entity_detail" => crate::entity_detail(a, iarg(b, "entityId")).await,
        "entity_profile" => crate::entity_profile(a, iarg(b, "entityId")).await,
        "list_people" => crate::list_people(a).await,
        "get_provider_settings" => Ok(crate::get_provider_settings()),
        "set_provider_settings" => crate::set_provider_settings(
            a,
            sarg(b, "mode"),
            oarg(b, "gemini_api_key"),
            oarg(b, "gemini_text_model"),
            oarg(b, "gemini_vision_model"),
        )
        .map(|_| Value::Null),
        "test_provider" => crate::test_provider().await.map(|s| json!(s)),
        "gcal_auth_status" => Ok(crate::gcal_auth_status()),
        "gcal_set_client" => {
            crate::gcal_set_client(a, sarg(b, "clientId"), sarg(b, "clientSecret")).map(|_| Value::Null)
        }
        "gcal_begin_auth" => crate::gcal_begin_auth(a).await,
        "gcal_disconnect" => crate::gcal_disconnect(a).map(|_| Value::Null),
        "gcal_sync" => crate::gcal_sync(a, oarg(b, "eventDate")).await,
        "gcal_clear_day" => crate::gcal_clear_day(a, oarg(b, "eventDate")).await.map(|n| json!(n)),
        "gcal_list_events" => crate::gcal_list_events(a, oarg(b, "eventDate")).await,
        "gcal_remove_account" => crate::gcal_remove_account(a, sarg(b, "email")),
        "gcal_set_calendar_enabled" => crate::gcal_set_calendar_enabled(
            a,
            sarg(b, "account"),
            sarg(b, "calendarId"),
            b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        ),
        "gcal_refresh_calendars" => crate::gcal_refresh_calendars(a).await,
        "gcal_set_sync_account" => crate::gcal_set_sync_account(a, sarg(b, "email")),
        "gcal_contacts" => Ok(crate::gcal_contacts()),
        "gcal_events_range" => {
            crate::gcal_events_range(a, sarg(b, "startDate"), sarg(b, "endDate")).await
        }
        "gcal_create_event" => crate::gcal_create_event(
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
            b.get("guests").and_then(|v| serde_json::from_value(v.clone()).ok()),
        )
        .await,
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
        "brain_remove_vault" => crate::brain_remove_vault(a, sarg(b, "vault")).await.map(|_| Value::Null),
        "brain_sync" => crate::brain_sync(a, oarg(b, "vault")).await,
        "work_graph" => crate::work_graph(a, oarg(b, "vault")).await,
        "brain_write_preview" => crate::brain_write_preview(a, oarg(b, "vault")).await,
        "brain_write_back" => crate::brain_write_back(a, oarg(b, "vault")).await,
        "personal_export_preview" => crate::personal_export_preview(a).await,
        "personal_export" => crate::personal_export(a).await,
        "related_brain" => crate::related_brain(a, sarg(b, "text")).await,
        "brain_get_auto" => Ok(json!(crate::brain_get_auto())),
        "brain_set_auto" => crate::brain_set_auto(a, b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true)).map(|_| Value::Null),
        other => Err(format!("unknown command: {other}")),
    }
}

// ── arg helpers (read frontend-shaped JSON keys) ────────────────────────────
fn sarg(b: &Value, k: &str) -> String {
    b.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string()
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
    // Serve the PWA manifest dynamically so its start_url carries the access
    // token. Without this, an installed iOS home-screen icon launches at "/"
    // with no token — and iOS standalone apps have their own empty localStorage,
    // so every /api call would 403. Baking the (persistent) token into start_url
    // means the installed app launches authenticated and stays that way.
    if path == "/manifest.webmanifest" {
        let token = app.state::<PhoneState>().token.clone();
        let _ = req.respond(
            tiny_http::Response::from_string(manifest_json(&token))
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
    if let Some(asset) = resolver.get(format!("/{rel}")).or_else(|| resolver.get(rel.to_string())) {
        // Tauri's mime guesser doesn't know .webmanifest — correct it so the
        // browser recognizes the PWA manifest (otherwise install is flaky).
        let ct = mime_override(rel).unwrap_or(asset.mime_type.as_str());
        let _ = req.respond(tiny_http::Response::from_data(asset.bytes).with_header(header("Content-Type", ct)));
        return;
    }

    // 2) Dev fallback: proxy the Vite dev server so the phone works in `tauri dev`.
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
            let _ = req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
    }
}

// PWA manifest with the token baked into start_url (see serve_static).
fn manifest_json(token: &str) -> String {
    json!({
        "name": "noted",
        "short_name": "noted",
        "description": "Your personal log — capture, search, and ask.",
        "start_url": format!("/?t={token}"),
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
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
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
