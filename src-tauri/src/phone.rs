// Phone capture: a tiny LAN HTTP server so you can snap a photo on your phone
// (same wifi) and have it flow straight into noted — no cable, no cloud.
//   GET  /         -> a mobile upload page (camera input)
//   POST /upload   -> saves the image to the inbox, emits "photo-received"
// Uploads are gated by a random token shown in the desktop app (as a QR/url).

use std::path::PathBuf;

use serde_json::json;
use tauri::{AppHandle, Emitter};

/// Connection info surfaced to the UI (url contains the token).
pub struct PhoneState {
    pub url: String,
    pub token: String,
    pub port: u16,
}

/// Bind a port (trying a few), returning the bound server + chosen port.
pub fn bind(preferred: u16) -> Option<(tiny_http::Server, u16)> {
    for port in [preferred, preferred + 1, preferred + 2] {
        if let Ok(s) = tiny_http::Server::http(("0.0.0.0", port)) {
            return Some((s, port));
        }
    }
    None
}

pub fn serve(server: tiny_http::Server, app: AppHandle, inbox: PathBuf, token: String) {
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let method = req.method().clone();
            if matches!(method, tiny_http::Method::Get) && !url.starts_with("/upload") {
                let _ = req.respond(html_response(PAGE));
                continue;
            }
            if matches!(method, tiny_http::Method::Post) && url.starts_with("/upload") {
                if !query_token_ok(&url, &token) {
                    let _ = req.respond(tiny_http::Response::from_string("forbidden").with_status_code(403));
                    continue;
                }
                let ext = content_type_ext(&req);
                let mut bytes = Vec::new();
                if req.as_reader().read_to_end(&mut bytes).is_err() || bytes.is_empty() {
                    let _ = req.respond(tiny_http::Response::from_string("bad").with_status_code(400));
                    continue;
                }
                match save_and_notify(&app, &inbox, &bytes, &ext) {
                    Ok(_) => {
                        let _ = req.respond(tiny_http::Response::from_string("ok"));
                    }
                    Err(e) => {
                        let _ = req.respond(tiny_http::Response::from_string(e).with_status_code(500));
                    }
                }
                continue;
            }
            let _ = req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
    });
}

fn save_and_notify(app: &AppHandle, inbox: &PathBuf, bytes: &[u8], ext: &str) -> Result<(), String> {
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
