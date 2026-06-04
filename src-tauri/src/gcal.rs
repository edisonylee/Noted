// Google Calendar one-way sync. noted pushes the day's schedule (the timed
// `blocks` the Today view shows) into a dedicated "noted" calendar in the user's
// Google account. It never pulls events back and never touches the primary
// calendar.
//
// Design mirrors provider.rs deliberately so there's one secret-handling story:
// - The OAuth client id + the synced calendar id live in a small JSON file in
//   the app data dir (gcal.json). Non-secret.
// - The OAuth *client secret* and the *refresh token* live in the macOS Keychain
//   (via the `security` CLI — no new crate), never on disk, never in the JSON.
// - The short-lived access token is cached in a process-global and refreshed on
//   demand; it's never persisted.
//
// Auth is the OAuth 2.0 "installed app" loopback flow: we bind an ephemeral
// 127.0.0.1 port with tiny_http, open the consent page in the browser, and catch
// the redirect locally. PKCE (S256) is used; Google still issues a client secret
// for desktop clients but it isn't treated as confidential.
//
// Idempotency: each timed block gets a *deterministic* Google event id derived
// from (event_date, block index), so re-syncing the same day updates the same
// events instead of creating duplicates. Events are tagged with a private
// `notedDate` extended property so a per-day sync can delete events for blocks
// that were removed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::Engine;
use chrono::{NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const KEYCHAIN_SERVICE: &str = "com.noted.app"; // shared with provider.rs
const ACCT_REFRESH: &str = "gcal_refresh_token";
const ACCT_SECRET: &str = "gcal_client_secret";
const CONFIG_FILE: &str = "gcal.json";

const AUTH_BASE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CAL_BASE: &str = "https://www.googleapis.com/calendar/v3";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar";
const CAL_SUMMARY: &str = "noted"; // the dedicated calendar's title

/// Schedule blocks are stored (and displayed) in Eastern wall-clock time — see
/// `today_local()` in lib.rs. Calendar datetimes must use the same zone so a
/// "9:00" block lands at 9:00 Eastern with the correct DST offset, not the
/// machine's local zone.
const TZ: chrono_tz::Tz = chrono_tz::America::New_York;

// ── Config ──────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GcalConfig {
    #[serde(default)]
    pub client_id: String, // non-secret OAuth client id
    #[serde(default)]
    pub calendar_id: Option<String>, // the dedicated "noted" calendar, set on bootstrap
    #[serde(default)]
    pub account_email: Option<String>, // display only (currently unused)
    // Loaded from the Keychain / minted at runtime; never serialized to JSON.
    #[serde(skip)]
    pub client_secret: Option<String>,
    #[serde(skip)]
    pub refresh_token: Option<String>,
    #[serde(skip)]
    pub access_token: Option<String>,
    #[serde(skip)]
    pub access_expires_at: Option<i64>, // unix secs
}

static CONFIG: OnceLock<RwLock<GcalConfig>> = OnceLock::new();
fn cell() -> &'static RwLock<GcalConfig> {
    CONFIG.get_or_init(|| RwLock::new(GcalConfig::default()))
}

/// Snapshot of the current config (cheap clone).
pub fn get() -> GcalConfig {
    cell().read().unwrap().clone()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn keychain_write(account: &str, value: &str) -> Result<()> {
    let status = Command::new("security")
        .args(["add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", account, "-w", value])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("failed to write {account} to keychain"))
    }
}

fn keychain_delete(account: &str) {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account])
        .status();
}

// ── Config file (client id + calendar id only — never secrets) ──────────────
fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

fn write_config_file(dir: &Path) {
    if let Ok(s) = serde_json::to_string_pretty(&get()) {
        let _ = std::fs::write(config_path(dir), s);
    }
}

/// Load config from disk + secrets from the Keychain into the process global.
/// Called once at startup, beside `provider::init`.
pub fn init(dir: &Path) {
    let mut c = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<GcalConfig>(&s).ok())
        .unwrap_or_default();
    c.client_secret = keychain_read(ACCT_SECRET);
    c.refresh_token = keychain_read(ACCT_REFRESH);
    *cell().write().unwrap() = c;
}

/// Store the user's OAuth client (id + secret) before they connect. Secret →
/// Keychain; id → in-memory + JSON.
pub fn save_client(dir: &Path, client_id: &str, client_secret: &str) -> Result<()> {
    if client_id.is_empty() || client_secret.is_empty() {
        return Err(anyhow!("both the client id and client secret are required"));
    }
    keychain_write(ACCT_SECRET, client_secret)?;
    {
        let mut c = cell().write().unwrap();
        c.client_id = client_id.to_string();
        c.client_secret = Some(client_secret.to_string());
    }
    write_config_file(dir);
    Ok(())
}

fn set_calendar_id(dir: &Path, id: &str) {
    {
        let mut c = cell().write().unwrap();
        c.calendar_id = Some(id.to_string());
    }
    write_config_file(dir);
}

/// Forget the Google session (refresh token gone/expired). Keeps the OAuth
/// client + calendar id so reconnecting doesn't require re-entering credentials.
fn clear_tokens(dir: &Path) {
    keychain_delete(ACCT_REFRESH);
    {
        let mut c = cell().write().unwrap();
        c.refresh_token = None;
        c.access_token = None;
        c.access_expires_at = None;
    }
    write_config_file(dir);
}

/// Connection status for the Settings UI — no network calls.
pub fn auth_status() -> Value {
    let c = get();
    json!({
        "connected": c.refresh_token.as_deref().map(|r| !r.is_empty()).unwrap_or(false),
        "has_client": !c.client_id.is_empty()
            && c.client_secret.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        "account_email": c.account_email,
        "calendar_id": c.calendar_id,
    })
}

/// Disconnect: drop the Google session but keep the OAuth client config.
pub fn disconnect(dir: &Path) {
    clear_tokens(dir);
}

// ── OAuth (installed-app loopback + PKCE S256) ──────────────────────────────
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?)
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn rand_token() -> String {
    format!("{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>())
}

/// Percent-encode a query value (everything outside the unreserved set).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = url.split('?').nth(1) {
        for pair in q.split('&') {
            let mut it = pair.splitn(2, '=');
            if let Some(k) = it.next() {
                map.insert(percent_decode(k), percent_decode(it.next().unwrap_or("")));
            }
        }
    }
    map
}

fn build_auth_url(client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    format!(
        "{AUTH_BASE}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent&state={}",
        enc(client_id),
        enc(redirect),
        enc(SCOPE),
        enc(challenge),
        enc(state),
    )
}

fn open_browser(url: &str) {
    let _ = Command::new("open").arg(url).spawn();
}

fn html_resp(body: &'static str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
    tiny_http::Response::from_string(body).with_header(header)
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>noted</title>\
<body style='font-family:-apple-system,system-ui,sans-serif;background:#0e0f13;color:#e7e9ee;\
display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
<div style='text-align:center'><h2>noted is connected ✓</h2>\
<p style='color:#8b90a0'>You can close this tab and return to noted.</p></div>";

const PAGE_ERR: &str = "<!doctype html><meta charset=utf-8><title>noted</title>\
<body style='font-family:-apple-system,system-ui,sans-serif;background:#0e0f13;color:#e7e9ee;\
display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
<div style='text-align:center'><h2>Connection failed</h2>\
<p style='color:#8b90a0'>Return to noted and try again.</p></div>";

/// Block on the loopback listener until the OAuth redirect arrives (or timeout),
/// validating `state` and returning the authorization `code`. Runs on a worker
/// thread (the recv is blocking).
fn wait_for_code(server: tiny_http::Server, want_state: &str) -> Result<String> {
    let start = std::time::Instant::now();
    let total = Duration::from_secs(180);
    loop {
        let remaining = total
            .checked_sub(start.elapsed())
            .ok_or_else(|| anyhow!("timed out waiting for Google authorization"))?;
        let req = match server.recv_timeout(remaining)? {
            Some(r) => r,
            None => return Err(anyhow!("timed out waiting for Google authorization")),
        };
        let url = req.url().to_string();
        // Browsers probe /favicon.ico; ignore and keep waiting for the redirect.
        if url.starts_with("/favicon") {
            let _ = req.respond(tiny_http::Response::from_string("").with_status_code(404));
            continue;
        }
        let params = parse_query(&url);
        if let Some(err) = params.get("error") {
            let _ = req.respond(html_resp(PAGE_ERR));
            return Err(anyhow!("authorization denied ({err})"));
        }
        let state_ok = params.get("state").map(|s| s == want_state).unwrap_or(false);
        match (state_ok, params.get("code")) {
            (true, Some(code)) => {
                let _ = req.respond(html_resp(PAGE_OK));
                return Ok(code.clone());
            }
            _ => {
                let _ = req.respond(html_resp(PAGE_ERR));
                return Err(anyhow!("invalid authorization response (state mismatch)"));
            }
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token: Option<String>,
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<TokenResponse> {
    let resp = http_client()?
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("token exchange failed ({status}): {text}"));
    }
    Ok(resp.json().await?)
}

/// Run the full consent flow: PKCE + loopback + code exchange, then persist the
/// refresh token. Returns the new auth status. Desktop-only (opens a browser on
/// the machine running noted).
pub async fn begin_auth(dir: &Path) -> Result<Value> {
    let (client_id, client_secret) = {
        let c = get();
        (
            c.client_id.clone(),
            c.client_secret.clone().unwrap_or_default(),
        )
    };
    if client_id.is_empty() || client_secret.is_empty() {
        return Err(anyhow!("add your Google OAuth client id and secret first"));
    }

    let verifier = b64url(&rand::random::<[u8; 32]>());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = rand_token();

    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow!("couldn't start the local auth listener: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("no loopback port"))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}");

    // Catch the redirect on a worker thread (recv is blocking), hand the code
    // back over a oneshot so we stay on the async runtime.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let want_state = state.clone();
    std::thread::spawn(move || {
        let _ = tx.send(wait_for_code(server, &want_state));
    });

    open_browser(&build_auth_url(&client_id, &redirect, &challenge, &state));

    let code = rx.await.map_err(|_| anyhow!("auth listener stopped unexpectedly"))??;
    let tokens = exchange_code(&client_id, &client_secret, &code, &verifier, &redirect).await?;
    let refresh = tokens.refresh_token.ok_or_else(|| {
        anyhow!("Google didn't return a refresh token — revoke noted's access in your Google account, then reconnect")
    })?;

    keychain_write(ACCT_REFRESH, &refresh)?;
    {
        let mut c = cell().write().unwrap();
        c.refresh_token = Some(refresh);
        c.access_token = Some(tokens.access_token);
        c.access_expires_at = Some(now_unix() + tokens.expires_in.unwrap_or(3600) - 60);
    }
    write_config_file(dir);
    Ok(auth_status())
}

/// A valid access token, refreshing via the stored refresh token when the cached
/// one is missing/expired. On `invalid_grant` (revoked/expired refresh token) it
/// clears the session and returns a "reconnect" error.
async fn get_access_token(dir: &Path) -> Result<String> {
    {
        let c = cell().read().unwrap();
        if let (Some(tok), Some(exp)) = (&c.access_token, c.access_expires_at) {
            if now_unix() < exp {
                return Ok(tok.clone());
            }
        }
    }
    refresh(dir).await
}

async fn refresh(dir: &Path) -> Result<String> {
    let (client_id, client_secret, refresh_token) = {
        let c = cell().read().unwrap();
        (
            c.client_id.clone(),
            c.client_secret.clone().unwrap_or_default(),
            c.refresh_token.clone(),
        )
    };
    let refresh_token =
        refresh_token.ok_or_else(|| anyhow!("not connected to Google Calendar"))?;

    let resp = http_client()?
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if text.contains("invalid_grant") {
            clear_tokens(dir);
            return Err(anyhow!(
                "Google Calendar disconnected — please reconnect (the authorization expired)."
            ));
        }
        return Err(anyhow!("token refresh failed ({status}): {text}"));
    }
    let t: TokenResponse = resp.json().await?;
    let access = t.access_token.clone();
    {
        let mut c = cell().write().unwrap();
        c.access_token = Some(access.clone());
        c.access_expires_at = Some(now_unix() + t.expires_in.unwrap_or(3600) - 60);
    }
    Ok(access)
}

// ── Calendar bootstrap ──────────────────────────────────────────────────────
/// The id of the dedicated "noted" calendar, creating it on first use. Reuses
/// the stored id, re-verifying it still exists (self-heals if the user deleted
/// the calendar), else finds one named "noted", else creates it.
async fn ensure_calendar(dir: &Path) -> Result<String> {
    let token = get_access_token(dir).await?;
    let client = http_client()?;

    if let Some(id) = get().calendar_id {
        let resp = client
            .get(format!("{CAL_BASE}/calendars/{}", enc(&id)))
            .bearer_auth(&token)
            .send()
            .await?;
        if resp.status().is_success() {
            return Ok(id);
        }
        // otherwise (404) fall through and re-resolve
    }

    let list = client
        .get(format!("{CAL_BASE}/users/me/calendarList"))
        .bearer_auth(&token)
        .send()
        .await?;
    if list.status().is_success() {
        let v: Value = list.json().await?;
        if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
            for it in items {
                if it.get("summary").and_then(|s| s.as_str()) == Some(CAL_SUMMARY) {
                    if let Some(id) = it.get("id").and_then(|s| s.as_str()) {
                        set_calendar_id(dir, id);
                        return Ok(id.to_string());
                    }
                }
            }
        }
    }

    let resp = client
        .post(format!("{CAL_BASE}/calendars"))
        .bearer_auth(&token)
        .json(&json!({ "summary": CAL_SUMMARY }))
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("couldn't create the noted calendar ({status}): {text}"));
    }
    let v: Value = resp.json().await?;
    let id = v
        .get("id")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("no calendar id in create response"))?
        .to_string();
    set_calendar_id(dir, &id);
    Ok(id)
}

// ── Sync ────────────────────────────────────────────────────────────────────
#[derive(Serialize, Default)]
pub struct SyncReport {
    pub created: u32,
    pub updated: u32,
    pub skipped: u32, // untimed ("Anytime") blocks, which have no clock time
    pub deleted: u32, // events for blocks that were removed since the last sync
    pub errors: Vec<String>,
}

/// Deterministic Google event id from (event_date, block index): same day+index
/// always maps to the same event, so re-sync updates instead of duplicating.
/// base32hex charset (a-v, 0-9); "noted" prefix + 32 hex chars = 37 chars.
fn event_id(event_date: &str, index: usize) -> String {
    let digest = Sha256::digest(format!("noted-{event_date}-{index}").as_bytes());
    const ALPHA: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(32);
    for b in &digest[..16] {
        s.push(ALPHA[(b >> 4) as usize] as char);
        s.push(ALPHA[(b & 0x0f) as usize] as char);
    }
    format!("noted{s}")
}

fn parse_hhmm(s: &str) -> Result<i64> {
    let (h, m) = s.split_once(':').ok_or_else(|| anyhow!("bad time {s}"))?;
    let h: i64 = h.trim().parse().map_err(|_| anyhow!("bad time {s}"))?;
    let m: i64 = m.trim().parse().map_err(|_| anyhow!("bad time {s}"))?;
    if h > 23 || m > 59 {
        return Err(anyhow!("bad time {s}"));
    }
    Ok(h * 60 + m)
}

/// RFC3339 (with Eastern offset) for `event_date` at midnight + `minutes`.
/// Minutes ≥ 1440 roll into the next day (handles cross-midnight blocks).
fn rfc3339_from_minutes(event_date: &str, minutes: i64) -> Result<String> {
    let d = NaiveDate::parse_from_str(event_date, "%Y-%m-%d")?;
    let naive = d
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("bad date {event_date}"))?
        + chrono::Duration::minutes(minutes);
    // Spring-forward gaps / fall-back ambiguity: nudge forward an hour so we
    // always get a concrete instant rather than failing.
    let dt = TZ
        .from_local_datetime(&naive)
        .single()
        .or_else(|| {
            TZ.from_local_datetime(&(naive + chrono::Duration::hours(1)))
                .single()
        })
        .ok_or_else(|| anyhow!("invalid local time"))?;
    Ok(dt.to_rfc3339())
}

/// One schedule block → a Google Calendar event body with our deterministic id
/// and the `notedDate` tag used for per-day stale cleanup.
fn build_event(event_date: &str, task: &str, start: &str, block: &Value, id: &str) -> Result<Value> {
    let smin = parse_hhmm(start)?;
    let emin = match block.get("end").and_then(|e| e.as_str()) {
        Some(end) => {
            let mut m = parse_hhmm(end)?;
            if m <= smin {
                m += 1440; // crosses midnight
            }
            m
        }
        None => match block.get("duration_min").and_then(|d| d.as_i64()) {
            Some(d) if d > 0 => smin + d,
            _ => smin + 60, // default 1h, matching the Today view's effEnd fallback
        },
    };
    // No `source` field: Google requires source.url to be a valid URL and
    // rejects the event otherwise. The notedDate tag is what we rely on.
    Ok(json!({
        "id": id,
        "summary": task,
        "start": { "dateTime": rfc3339_from_minutes(event_date, smin)? },
        "end": { "dateTime": rfc3339_from_minutes(event_date, emin)? },
        "extendedProperties": { "private": { "notedDate": event_date } },
    }))
}

/// Insert the event; if it already exists (409), update it. Ok(true)=created,
/// Ok(false)=updated.
async fn upsert_event(
    client: &reqwest::Client,
    token: &str,
    cal: &str,
    id: &str,
    body: &Value,
) -> Result<bool> {
    let resp = client
        .post(format!("{CAL_BASE}/calendars/{}/events", enc(cal)))
        .bearer_auth(token)
        .json(body)
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(true);
    }
    if resp.status().as_u16() == 409 {
        let resp = client
            .put(format!("{CAL_BASE}/calendars/{}/events/{}", enc(cal), enc(id)))
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        if resp.status().is_success() {
            return Ok(false);
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("update failed ({status}): {text}"));
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    Err(anyhow!("insert failed ({status}): {text}"))
}

/// Ids of all noted-created events on `event_date` (matched by the private
/// `notedDate` tag), for deciding which to delete.
async fn list_day_event_ids(
    client: &reqwest::Client,
    token: &str,
    cal: &str,
    event_date: &str,
) -> Result<Vec<String>> {
    let prop = enc(&format!("notedDate={event_date}"));
    let resp = client
        .get(format!(
            "{CAL_BASE}/calendars/{}/events?privateExtendedProperty={}&maxResults=2500&showDeleted=false",
            enc(cal),
            prop,
        ))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("list failed ({status}): {text}"));
    }
    let v: Value = resp.json().await?;
    Ok(v.get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| it.get("id").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

async fn delete_event(client: &reqwest::Client, token: &str, cal: &str, id: &str) -> Result<()> {
    let resp = client
        .delete(format!("{CAL_BASE}/calendars/{}/events/{}", enc(cal), enc(id)))
        .bearer_auth(token)
        .send()
        .await?;
    let code = resp.status().as_u16();
    // 200/204 = deleted; 404/410 = already gone — both are success for our purpose.
    if resp.status().is_success() || code == 404 || code == 410 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_default();
        Err(anyhow!("{code}: {text}"))
    }
}

/// Push one day's schedule `blocks` to the dedicated calendar. Idempotent:
/// re-running updates existing events and removes events for dropped blocks.
/// Untimed blocks are skipped (counted in the report). Best-effort: per-block
/// failures are collected into `errors` rather than aborting the whole sync.
pub async fn sync(dir: &Path, event_date: &str, blocks: Vec<Value>) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let token = get_access_token(dir).await?;
    let cal = ensure_calendar(dir).await?;
    let client = http_client()?;

    let mut fresh_ids: Vec<String> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let task = block.get("task").and_then(|t| t.as_str()).unwrap_or("").trim();
        let start = block.get("start").and_then(|s| s.as_str());
        match (task.is_empty(), start) {
            (false, Some(start)) => {
                let id = event_id(event_date, i);
                match build_event(event_date, task, start, block, &id) {
                    Ok(body) => {
                        fresh_ids.push(id.clone());
                        match upsert_event(&client, &token, &cal, &id, &body).await {
                            Ok(true) => report.created += 1,
                            Ok(false) => report.updated += 1,
                            Err(e) => report.errors.push(format!("{task}: {e}")),
                        }
                    }
                    Err(e) => report.errors.push(format!("{task}: {e}")),
                }
            }
            // Untimed ("Anytime") block, or empty task — not a calendar event.
            _ => report.skipped += 1,
        }
    }

    // Per-day "replace": delete any noted events for this date no longer present.
    match list_day_event_ids(&client, &token, &cal, event_date).await {
        Ok(existing) => {
            for id in existing {
                if !fresh_ids.contains(&id) {
                    match delete_event(&client, &token, &cal, &id).await {
                        Ok(()) => report.deleted += 1,
                        Err(e) => report.errors.push(format!("delete {id}: {e}")),
                    }
                }
            }
        }
        Err(e) => report.errors.push(format!("stale cleanup: {e}")),
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_is_deterministic_and_valid() {
        let a = event_id("2026-06-04", 0);
        let b = event_id("2026-06-04", 0);
        assert_eq!(a, b, "same date+index must yield the same id");
        assert_ne!(event_id("2026-06-04", 0), event_id("2026-06-04", 1));
        assert_ne!(event_id("2026-06-04", 0), event_id("2026-06-05", 0));
        // base32hex charset: a-v and 0-9 only, length within Google's 5..=1024.
        assert!(a.len() >= 5 && a.len() <= 1024);
        assert!(
            a.chars().all(|c| matches!(c, 'a'..='v' | '0'..='9')),
            "id {a} must be base32hex"
        );
    }

    #[test]
    fn rfc3339_uses_eastern_offset_and_dst() {
        // June → EDT (-04:00)
        let summer = rfc3339_from_minutes("2026-06-04", 9 * 60).unwrap();
        assert!(summer.starts_with("2026-06-04T09:00:00-04:00"), "{summer}");
        // January → EST (-05:00)
        let winter = rfc3339_from_minutes("2026-01-04", 9 * 60).unwrap();
        assert!(winter.starts_with("2026-01-04T09:00:00-05:00"), "{winter}");
    }

    #[test]
    fn rfc3339_rolls_past_midnight() {
        // 23:30 + 60min worth of carry (1530 min from midnight) → next day 01:30
        let dt = rfc3339_from_minutes("2026-06-04", 25 * 60 + 30).unwrap();
        assert!(dt.starts_with("2026-06-05T01:30:00"), "{dt}");
    }

    #[test]
    fn build_event_defaults_end_to_one_hour() {
        let block = json!({ "task": "gym", "start": "09:00" });
        let ev = build_event("2026-06-04", "gym", "09:00", &block, "notedabc").unwrap();
        assert!(ev["start"]["dateTime"].as_str().unwrap().starts_with("2026-06-04T09:00:00"));
        assert!(ev["end"]["dateTime"].as_str().unwrap().starts_with("2026-06-04T10:00:00"));
        assert_eq!(ev["extendedProperties"]["private"]["notedDate"], "2026-06-04");
    }

    #[test]
    fn build_event_crosses_midnight_when_end_before_start() {
        let block = json!({ "task": "sleep", "start": "23:00", "end": "06:00" });
        let ev = build_event("2026-06-04", "sleep", "23:00", &block, "notedxyz").unwrap();
        assert!(ev["start"]["dateTime"].as_str().unwrap().starts_with("2026-06-04T23:00:00"));
        assert!(ev["end"]["dateTime"].as_str().unwrap().starts_with("2026-06-05T06:00:00"));
    }
}
