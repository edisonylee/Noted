// Google Calendar integration. Two directions:
// - Push: noted pushes the day's schedule (the timed `blocks` the Today view
//   shows) into a dedicated "noted" calendar in the *first* connected account.
// - Pull: the Calendar view reads events across EVERY connected account's
//   enabled calendars (multi-account, so work + personal consolidate in one
//   grid), and can create/edit/delete events on any of them.
//
// Design mirrors provider.rs deliberately so there's one secret-handling story:
// - The OAuth client id, the synced calendar id, and each account's calendar
//   list (names/colors/visibility) live in a small JSON file in the app data
//   dir (gcal.json). Non-secret.
// - The OAuth *client secret* and each account's *refresh token* live in the
//   macOS Keychain (via the `security` CLI — no new crate), never on disk,
//   never in the JSON. Refresh tokens are keyed per account email.
// - Short-lived access tokens are cached per account in a process-global and
//   refreshed on demand; never persisted.
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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::Engine;
use chrono::{NaiveDate, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const KEYCHAIN_SERVICE: &str = "com.noted.app"; // shared with provider.rs
const ACCT_REFRESH: &str = "gcal_refresh_token"; // legacy single-account key (migrated at init)
const ACCT_SECRET: &str = "gcal_client_secret";
const CONFIG_FILE: &str = "gcal.json";

/// Keychain account name for one Google account's refresh token.
fn refresh_key(email: &str) -> String {
    format!("{ACCT_REFRESH}:{email}")
}

const AUTH_BASE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CAL_BASE: &str = "https://www.googleapis.com/calendar/v3";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar";
const CAL_SUMMARY: &str = "noted"; // the dedicated calendar's title

/// Calendar datetimes use the same user-selected zone as `today_local()` in
/// lib.rs, so a "9:00" block stays at 9:00 in the app across DST changes.

// ── Config ──────────────────────────────────────────────────────────────────
fn default_true() -> bool {
    true
}

/// One calendar inside an account, as cached in gcal.json. `enabled` is the
/// user's show/hide choice for the Calendar view; names/colors refresh from
/// Google, the enabled flag is ours and survives refreshes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GcalCalendar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: String, // Google's backgroundColor hex
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub access: String, // Google accessRole: owner/writer/reader/freeBusyReader
}

/// One connected Google account. The email doubles as the identity key: it
/// names the Keychain entry holding the refresh token and is what commands
/// pass to pick an account. Tokens never serialize to JSON.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GcalAccount {
    pub email: String,
    #[serde(default)]
    pub calendars: Vec<GcalCalendar>,
    #[serde(skip)]
    pub refresh_token: Option<String>,
    #[serde(skip)]
    pub access_token: Option<String>,
    #[serde(skip)]
    pub access_expires_at: Option<i64>, // unix secs
}

/// One remembered guest — the autocomplete pool for the event form. Built
/// locally from attendees seen on calendar events (no Google contacts scope).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GcalContact {
    pub email: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GcalConfig {
    #[serde(default)]
    pub client_id: String, // non-secret OAuth client id, shared by every account
    #[serde(default)]
    pub calendar_id: Option<String>, // the dedicated "noted" calendar, set on bootstrap
    #[serde(default)]
    pub account_email: Option<String>, // legacy pre-multi-account field; migrated at init
    #[serde(default)]
    pub accounts: Vec<GcalAccount>,
    #[serde(default)]
    pub sync_account: Option<String>, // hosts the "noted" push calendar; None = first account
    #[serde(default)]
    pub contacts: Vec<GcalContact>, // guest-autocomplete pool, harvested from events
    // Loaded from the Keychain / minted at runtime; never serialized to JSON.
    #[serde(skip)]
    pub client_secret: Option<String>,
}

static CONFIG: OnceLock<RwLock<GcalConfig>> = OnceLock::new();
fn cell() -> &'static RwLock<GcalConfig> {
    CONFIG.get_or_init(|| RwLock::new(GcalConfig::default()))
}

/// Snapshot of the current config (cheap clone).
pub fn get() -> GcalConfig {
    cell().read().unwrap().clone()
}

fn configured_account_emails_from(config: &GcalConfig) -> Vec<String> {
    let mut emails = config
        .accounts
        .iter()
        .map(|account| account.email.trim().to_lowercase())
        .filter(|email| !email.is_empty())
        .collect::<Vec<_>>();
    emails.sort();
    emails.dedup();
    emails
}

/// Exact identities remembered in Google Calendar config. An expired session
/// does not make a participant external; removing the account is the explicit
/// boundary for forgetting that identity.
pub fn configured_account_emails() -> Vec<String> {
    configured_account_emails_from(&cell().read().unwrap())
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
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
            "-w",
        ])
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
        .args([
            "add-generic-password",
            "-U",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
            "-w",
            value,
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("failed to write {account} to keychain"))
    }
}

fn keychain_delete(account: &str) {
    let _ = Command::new("security")
        .args([
            "delete-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            account,
        ])
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
/// Called once at startup, beside `provider::init`. Migrates the legacy
/// single-account layout (one un-keyed refresh token) into accounts[0].
pub fn init(dir: &Path) {
    let mut c = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<GcalConfig>(&s).ok())
        .unwrap_or_default();
    c.client_secret = keychain_read(ACCT_SECRET);

    // Legacy → multi-account: move the un-keyed refresh token under an
    // email-keyed entry so it behaves like any other account from here on.
    let mut migrated = false;
    if c.accounts.is_empty() {
        if let Some(tok) = keychain_read(ACCT_REFRESH) {
            let email = c
                .account_email
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "google-account".to_string());
            if keychain_write(&refresh_key(&email), &tok).is_ok() {
                keychain_delete(ACCT_REFRESH);
                c.accounts.push(GcalAccount {
                    email,
                    ..Default::default()
                });
                migrated = true;
            }
        }
    }

    for a in &mut c.accounts {
        a.refresh_token = keychain_read(&refresh_key(&a.email));
    }
    *cell().write().unwrap() = c;
    if migrated {
        write_config_file(dir);
    }
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

/// Forget ONE account's Google session (refresh token gone/expired). The
/// account entry and its calendar list stay, so the UI can offer "reconnect"
/// and the user's visibility choices survive.
fn clear_account_session(dir: &Path, email: &str) {
    keychain_delete(&refresh_key(email));
    {
        let mut c = cell().write().unwrap();
        if let Some(a) = c.accounts.iter_mut().find(|a| a.email == email) {
            a.refresh_token = None;
            a.access_token = None;
            a.access_expires_at = None;
        }
    }
    write_config_file(dir);
}

/// Remove an account entirely: token out of the Keychain, entry out of the
/// config. The OAuth client (id + secret) stays so re-adding is one click.
pub fn remove_account(dir: &Path, email: &str) -> Result<Value> {
    let email = email.trim().to_lowercase();
    keychain_delete(&refresh_key(&email));
    {
        let mut c = cell().write().unwrap();
        c.accounts
            .retain(|a| a.email.trim().to_lowercase() != email);
        c.account_email = c.accounts.first().map(|a| a.email.clone());
    }
    write_config_file(dir);
    Ok(auth_status())
}

/// Connection status for Settings + the Calendar view — no network calls.
pub fn auth_status() -> Value {
    let c = get();
    let connected = |a: &GcalAccount| {
        a.refresh_token
            .as_deref()
            .map(|r| !r.is_empty())
            .unwrap_or(false)
    };
    json!({
        "connected": c.accounts.iter().any(connected),
        "has_client": !c.client_id.is_empty()
            && c.client_secret.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        "account_email": c.accounts.first().map(|a| a.email.clone()),
        "sync_account": resolve_sync_account(&c.accounts, c.sync_account.as_deref()),
        "calendar_id": c.calendar_id,
        "accounts": c.accounts.iter().map(|a| json!({
            "email": a.email,
            "connected": connected(a),
            "calendars": a.calendars.iter().map(|k| json!({
                "id": k.id,
                "name": k.name,
                "color": k.color,
                "enabled": k.enabled,
                "primary": k.primary,
                "access": k.access,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Disconnect everything: every account's session and entry. Keeps the OAuth
/// client config so reconnecting doesn't require re-entering credentials.
pub fn disconnect(dir: &Path) {
    let emails: Vec<String> = get().accounts.iter().map(|a| a.email.clone()).collect();
    for e in emails {
        keychain_delete(&refresh_key(&e));
    }
    keychain_delete(ACCT_REFRESH); // legacy key, in case migration never ran
    {
        let mut c = cell().write().unwrap();
        c.accounts.clear();
        c.account_email = None;
    }
    write_config_file(dir);
}

/// Clear one day: delete only the events noted pushed for `event_date` (matched
/// by the private `notedDate` tag) from noted's own calendar. The calendar
/// itself, other days, every other calendar, and the Google session are all left
/// intact. Returns the number of events deleted. No-op (Ok(0)) if there's no
/// noted calendar yet — we never create one just to clear it.
pub async fn clear_day(dir: &Path, event_date: &str) -> Result<u32> {
    let token = get_access_token(dir).await?;
    let client = http_client()?;

    let cal = match find_calendar(&client, &token, get().calendar_id).await? {
        Some(id) => {
            // Re-pin the resolved id (self-heal) so a later sync reuses it.
            set_calendar_id(dir, &id);
            id
        }
        None => return Ok(0),
    };

    let mut deleted = 0u32;
    for id in list_day_event_ids(&client, &token, &cal, event_date).await? {
        // delete_event treats 404/410 as success, so a concurrent delete is fine.
        delete_event(&client, &token, &cal, &id).await?;
        deleted += 1;
    }
    Ok(deleted)
}

// ── OAuth (installed-app loopback + PKCE S256) ──────────────────────────────
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?)
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn rand_token() -> String {
    format!(
        "{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>()
    )
}

/// Percent-encode a query value (everything outside the unreserved set).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
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
    // `select_account` keeps Google from silently reusing the last session, so
    // adding a second (work) account actually shows the account picker.
    format!(
        "{AUTH_BASE}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&access_type=offline&prompt={}&state={}",
        enc(client_id),
        enc(redirect),
        enc(SCOPE),
        enc(challenge),
        enc("consent select_account"),
        enc(state),
    )
}

fn open_browser(url: &str) {
    let _ = Command::new("open").arg(url).spawn();
}

fn html_resp(body: &'static str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
            .unwrap();
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
        let state_ok = params
            .get("state")
            .map(|s| s == want_state)
            .unwrap_or(false);
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
/// account. This IS the "add account" flow — each run connects (or reconnects)
/// whichever Google account the user picks in the browser, keyed by its email.
/// Returns the new auth status. Desktop-only (opens a browser on the machine
/// running noted).
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

    let code = rx
        .await
        .map_err(|_| anyhow!("auth listener stopped unexpectedly"))??;
    let tokens = exchange_code(&client_id, &client_secret, &code, &verifier, &redirect).await?;
    let refresh = tokens.refresh_token.ok_or_else(|| {
        anyhow!("Google didn't return a refresh token — revoke noted's access in your Google account, then reconnect")
    })?;
    let access = tokens.access_token;
    let expires_at = now_unix() + tokens.expires_in.unwrap_or(3600) - 60;

    // The account's email is its identity key (Keychain entry, command arg), so
    // resolving it is a hard requirement of the add — it's the id of the
    // `primary` calendar in the calendar list we need anyway.
    let cals = fetch_calendars(&http_client()?, &access).await?;
    let email = cals
        .iter()
        .find(|c| c.primary)
        .map(|c| c.id.clone())
        .ok_or_else(|| anyhow!("couldn't identify the Google account (no primary calendar)"))?;

    keychain_write(&refresh_key(&email), &refresh)?;
    {
        let mut c = cell().write().unwrap();
        match c.accounts.iter_mut().find(|a| a.email == email) {
            // Reconnect: new tokens, refreshed calendar list, visibility kept.
            Some(a) => {
                a.refresh_token = Some(refresh);
                a.access_token = Some(access);
                a.access_expires_at = Some(expires_at);
                a.calendars = merge_calendars(std::mem::take(&mut a.calendars), cals);
            }
            None => c.accounts.push(GcalAccount {
                email: email.clone(),
                calendars: cals,
                refresh_token: Some(refresh),
                access_token: Some(access),
                access_expires_at: Some(expires_at),
            }),
        }
        c.account_email = c.accounts.first().map(|a| a.email.clone());
    }
    write_config_file(dir);
    Ok(auth_status())
}

/// Which account hosts the "noted" push calendar: the user's explicit choice
/// when it still exists, else the first connected. Pure so it's testable.
fn resolve_sync_account(accounts: &[GcalAccount], pref: Option<&str>) -> Option<String> {
    pref.and_then(|p| accounts.iter().find(|a| a.email == p))
        .or_else(|| accounts.first())
        .map(|a| a.email.clone())
}

/// The account the one-way schedule sync pushes through.
fn sync_account() -> Result<String> {
    let c = get();
    resolve_sync_account(&c.accounts, c.sync_account.as_deref())
        .ok_or_else(|| anyhow!("not connected to Google Calendar"))
}

/// Pick which account the daily-schedule push targets. The "noted" calendar
/// self-heals on the next sync: the stored id won't resolve under the new
/// account's token, so it's re-found by name or freshly created there.
pub fn set_sync_account(dir: &Path, email: &str) -> Result<Value> {
    {
        let mut c = cell().write().unwrap();
        if !c.accounts.iter().any(|a| a.email == email) {
            return Err(anyhow!("no such account: {email}"));
        }
        c.sync_account = Some(email.to_string());
    }
    write_config_file(dir);
    Ok(auth_status())
}

/// The guest-autocomplete pool for the event form.
pub fn contacts() -> Value {
    json!(get()
        .contacts
        .iter()
        .map(|k| json!({ "email": k.email, "name": k.name }))
        .collect::<Vec<_>>())
}

/// A valid access token for the sync account (schedule push / clear).
async fn get_access_token(dir: &Path) -> Result<String> {
    let email = sync_account()?;
    access_token_for(dir, &email).await
}

/// A valid access token for one account, refreshing via its stored refresh
/// token when the cached one is missing/expired. On `invalid_grant`
/// (revoked/expired refresh token) it clears that account's session — others
/// keep working — and returns a "reconnect" error.
async fn access_token_for(dir: &Path, email: &str) -> Result<String> {
    {
        let c = cell().read().unwrap();
        if let Some(a) = c.accounts.iter().find(|a| a.email == email) {
            if let (Some(tok), Some(exp)) = (&a.access_token, a.access_expires_at) {
                if now_unix() < exp {
                    return Ok(tok.clone());
                }
            }
        }
    }
    refresh_access(dir, email).await
}

async fn refresh_access(dir: &Path, email: &str) -> Result<String> {
    let (client_id, client_secret, refresh_token) = {
        let c = cell().read().unwrap();
        (
            c.client_id.clone(),
            c.client_secret.clone().unwrap_or_default(),
            c.accounts
                .iter()
                .find(|a| a.email == email)
                .and_then(|a| a.refresh_token.clone()),
        )
    };
    let refresh_token = refresh_token
        .ok_or_else(|| anyhow!("{email} isn't connected — add the account again in Settings"))?;

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
            clear_account_session(dir, email);
            return Err(anyhow!(
                "{email} disconnected — please reconnect it (the authorization expired)."
            ));
        }
        return Err(anyhow!(
            "token refresh failed for {email} ({status}): {text}"
        ));
    }
    let t: TokenResponse = resp.json().await?;
    let access = t.access_token.clone();
    {
        let mut c = cell().write().unwrap();
        if let Some(a) = c.accounts.iter_mut().find(|a| a.email == email) {
            a.access_token = Some(access.clone());
            a.access_expires_at = Some(now_unix() + t.expires_in.unwrap_or(3600) - 60);
        }
    }
    Ok(access)
}

// ── Calendar lists (per account) ────────────────────────────────────────────
/// Fetch an account's calendar list from Google: id, display name, color,
/// primary flag. Everything starts `enabled`; merge_calendars re-applies the
/// user's hide choices.
async fn fetch_calendars(client: &reqwest::Client, token: &str) -> Result<Vec<GcalCalendar>> {
    let resp = client
        .get(format!("{CAL_BASE}/users/me/calendarList?maxResults=250"))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("calendar list failed ({status}): {text}"));
    }
    let v: Value = resp.json().await?;
    Ok(v.get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c.get("id").and_then(|s| s.as_str())?;
                    // summaryOverride is the user's own rename of a shared calendar.
                    let name = c
                        .get("summaryOverride")
                        .or_else(|| c.get("summary"))
                        .and_then(|s| s.as_str())
                        .unwrap_or(id);
                    Some(GcalCalendar {
                        id: id.to_string(),
                        name: name.to_string(),
                        color: c
                            .get("backgroundColor")
                            .and_then(|s| s.as_str())
                            .unwrap_or("#039be5")
                            .to_string(),
                        enabled: true,
                        primary: c.get("primary").and_then(|p| p.as_bool()).unwrap_or(false),
                        access: c
                            .get("accessRole")
                            .and_then(|s| s.as_str())
                            .unwrap_or("reader")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Refresh a calendar list from Google while keeping the user's hide choices:
/// names/colors/membership come from `fresh`, `enabled=false` carries over by id.
fn merge_calendars(old: Vec<GcalCalendar>, fresh: Vec<GcalCalendar>) -> Vec<GcalCalendar> {
    let hidden: HashSet<String> = old
        .iter()
        .filter(|c| !c.enabled)
        .map(|c| c.id.clone())
        .collect();
    fresh
        .into_iter()
        .map(|mut c| {
            if hidden.contains(&c.id) {
                c.enabled = false;
            }
            c
        })
        .collect()
}

/// An account's calendars — cached list if present, else fetched from Google
/// and persisted (first use after the legacy migration lands here). A cache
/// written before the `access` field existed (every role empty) can't drive
/// the writable-calendar picker, so it re-fetches once to backfill, keeping
/// the user's show/hide choices.
async fn ensure_calendars(dir: &Path, email: &str) -> Result<Vec<GcalCalendar>> {
    let cached: Vec<GcalCalendar> = {
        let c = cell().read().unwrap();
        c.accounts
            .iter()
            .find(|a| a.email == email)
            .map(|a| a.calendars.clone())
            .unwrap_or_default()
    };
    let pre_access = !cached.is_empty() && cached.iter().all(|c| c.access.is_empty());
    if !cached.is_empty() && !pre_access {
        return Ok(cached);
    }
    let token = access_token_for(dir, email).await?;
    let fresh = fetch_calendars(&http_client()?, &token).await?;
    let merged = merge_calendars(cached, fresh);
    {
        let mut c = cell().write().unwrap();
        if let Some(a) = c.accounts.iter_mut().find(|a| a.email == email) {
            a.calendars = merged.clone();
        }
    }
    write_config_file(dir);
    Ok(merged)
}

/// Show/hide one calendar in the Calendar view. Persisted, survives refreshes.
pub fn set_calendar_enabled(
    dir: &Path,
    email: &str,
    calendar_id: &str,
    enabled: bool,
) -> Result<Value> {
    {
        let mut c = cell().write().unwrap();
        let a = c
            .accounts
            .iter_mut()
            .find(|a| a.email == email)
            .ok_or_else(|| anyhow!("no such account: {email}"))?;
        let k = a
            .calendars
            .iter_mut()
            .find(|k| k.id == calendar_id)
            .ok_or_else(|| anyhow!("no such calendar on {email}"))?;
        k.enabled = enabled;
    }
    write_config_file(dir);
    Ok(auth_status())
}

/// Re-pull every connected account's calendar list (new calendars, renames,
/// color changes). Best-effort per account: one broken session doesn't block
/// the rest — its `connected` flag flips in the returned status instead.
pub async fn refresh_calendars(dir: &Path) -> Result<Value> {
    let emails: Vec<String> = get()
        .accounts
        .iter()
        .filter(|a| a.refresh_token.is_some())
        .map(|a| a.email.clone())
        .collect();
    let client = http_client()?;
    for email in emails {
        let Ok(token) = access_token_for(dir, &email).await else {
            continue;
        };
        let Ok(fresh) = fetch_calendars(&client, &token).await else {
            continue;
        };
        let mut c = cell().write().unwrap();
        if let Some(a) = c.accounts.iter_mut().find(|a| a.email == email) {
            a.calendars = merge_calendars(std::mem::take(&mut a.calendars), fresh);
        }
    }
    write_config_file(dir);
    Ok(auth_status())
}

// ── Calendar bootstrap ──────────────────────────────────────────────────────
/// Find the dedicated "noted" calendar's id WITHOUT creating one: the stored id
/// if it still exists (self-heals if the user deleted it), else one named
/// "noted" in the account, else None.
async fn find_calendar(
    client: &reqwest::Client,
    token: &str,
    stored: Option<String>,
) -> Result<Option<String>> {
    if let Some(id) = stored {
        let resp = client
            .get(format!("{CAL_BASE}/calendars/{}", enc(&id)))
            .bearer_auth(token)
            .send()
            .await?;
        if resp.status().is_success() {
            return Ok(Some(id));
        }
        // otherwise (404) fall through and re-resolve by name
    }

    let list = client
        .get(format!("{CAL_BASE}/users/me/calendarList"))
        .bearer_auth(token)
        .send()
        .await?;
    if list.status().is_success() {
        let v: Value = list.json().await?;
        if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
            for it in items {
                if it.get("summary").and_then(|s| s.as_str()) == Some(CAL_SUMMARY) {
                    if let Some(id) = it.get("id").and_then(|s| s.as_str()) {
                        return Ok(Some(id.to_string()));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// The id of the dedicated "noted" calendar, creating it on first use. Reuses
/// the stored id, re-verifying it still exists (self-heals if the user deleted
/// the calendar), else finds one named "noted", else creates it.
async fn ensure_calendar(dir: &Path) -> Result<String> {
    let token = get_access_token(dir).await?;
    let client = http_client()?;

    if let Some(id) = find_calendar(&client, &token, get().calendar_id).await? {
        set_calendar_id(dir, &id);
        return Ok(id);
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
        return Err(anyhow!(
            "couldn't create the noted calendar ({status}): {text}"
        ));
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
    pub skipped: u32,    // untimed ("Anytime") blocks, which have no clock time
    pub duplicates: u32, // blocks already on another calendar at the same start+end
    pub deleted: u32,    // events for blocks that were removed since the last sync
    pub errors: Vec<String>,
}

/// Deterministic Google event id from the block's *content* (date, start, task):
/// the same block always maps to the same event, so re-sync updates instead of
/// duplicating — and, unlike an index-based key, reordering or deleting sibling
/// blocks can't re-point an id at a different block's event (the old id simply
/// drops out of `fresh_ids` and the stale-cleanup pass removes it). End time is
/// deliberately excluded so extending a block updates its event in place.
/// base32hex charset (a-v, 0-9); "noted" prefix + 32 hex chars = 37 chars.
fn event_id(event_date: &str, task: &str, start: &str) -> String {
    let digest = Sha256::digest(format!("noted-{event_date}-{start}-{}", task.trim()).as_bytes());
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

/// RFC3339 (with the configured zone's offset) for `event_date` at midnight + `minutes`.
/// Minutes ≥ 1440 roll into the next day (handles cross-midnight blocks).
fn rfc3339_from_minutes(event_date: &str, minutes: i64) -> Result<String> {
    let d = NaiveDate::parse_from_str(event_date, "%Y-%m-%d")?;
    let naive = d
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("bad date {event_date}"))?
        + chrono::Duration::minutes(minutes);
    // Spring-forward gaps / fall-back ambiguity: nudge forward an hour so we
    // always get a concrete instant rather than failing.
    let time_zone = crate::system_settings::time_zone();
    let dt = time_zone
        .from_local_datetime(&naive)
        .single()
        .or_else(|| {
            time_zone
                .from_local_datetime(&(naive + chrono::Duration::hours(1)))
                .single()
        })
        .ok_or_else(|| anyhow!("invalid local time"))?;
    Ok(dt.to_rfc3339())
}

/// Wall-clock "HH:MM" for a minutes-since-midnight value, wrapping past midnight
/// so it matches the read-back form from `list_events` for duplicate detection.
fn hhmm_from_minutes(min: i64) -> String {
    let m = min.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// A block's effective end in minutes-since-midnight, mirroring `build_event`:
/// explicit `end` (rolled past midnight if it's ≤ start), else `duration_min`,
/// else a default of one hour.
fn block_end_minutes(smin: i64, block: &Value) -> Result<i64> {
    Ok(match block.get("end").and_then(|e| e.as_str()) {
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
    })
}

/// One schedule block → a Google Calendar event body with our deterministic id
/// and the `notedDate` tag used for per-day stale cleanup.
fn build_event(
    event_date: &str,
    task: &str,
    start: &str,
    block: &Value,
    id: &str,
) -> Result<Value> {
    let smin = parse_hhmm(start)?;
    let emin = block_end_minutes(smin, block)?;
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
            .put(format!(
                "{CAL_BASE}/calendars/{}/events/{}",
                enc(cal),
                enc(id)
            ))
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
        .delete(format!(
            "{CAL_BASE}/calendars/{}/events/{}",
            enc(cal),
            enc(id)
        ))
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

    // Time windows already occupied on the user's *other* calendars, so we don't
    // duplicate a slot they've already booked. Best-effort: if the read-back
    // fails, fall back to an empty set (push everything) rather than dropping the
    // schedule, and note the failure in the report.
    let busy: HashSet<(String, String)> = match list_events(dir, event_date).await {
        Ok(events) => events
            .iter()
            .filter(|e| !e.get("all_day").and_then(|a| a.as_bool()).unwrap_or(false))
            .filter_map(|e| {
                let s = e.get("start").and_then(|s| s.as_str())?;
                let en = e.get("end").and_then(|s| s.as_str())?;
                Some((s.to_string(), en.to_string()))
            })
            .collect(),
        Err(e) => {
            report.errors.push(format!("duplicate check skipped: {e}"));
            HashSet::new()
        }
    };

    let mut fresh_ids: Vec<String> = Vec::new();
    for block in blocks.iter() {
        let task = block
            .get("task")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        let start = block.get("start").and_then(|s| s.as_str());
        match (task.is_empty(), start) {
            (false, Some(start)) => {
                // Skip blocks that match an existing event's exact start+end on
                // another calendar (regardless of title). The id is left out of
                // `fresh_ids`, so the stale-cleanup pass below also removes any
                // event noted pushed for this slot before the user booked it.
                if let Ok(smin) = parse_hhmm(start) {
                    if let Ok(emin) = block_end_minutes(smin, block) {
                        let window = (hhmm_from_minutes(smin), hhmm_from_minutes(emin));
                        if busy.contains(&window) {
                            report.duplicates += 1;
                            continue;
                        }
                    }
                }
                let id = event_id(event_date, task, start);
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

// ── Range read (Calendar view) ──────────────────────────────────────────────
/// Domains that mean "this URL joins a meeting" when found in an event's
/// location or description (where Zoom/Teams invites usually land).
const MEET_DOMAINS: [&str; 8] = [
    "zoom.us",
    "meet.google.com",
    "teams.microsoft.com",
    "webex.com",
    "whereby.com",
    "meet.jit.si",
    "discord.com",
    "discord.gg",
];

/// First conferencing URL inside free text (which may be HTML — Google event
/// descriptions often are, so URLs can sit inside href attributes).
fn find_meeting_url(text: &str) -> Option<String> {
    for (i, _) in text.match_indices("http") {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')'))
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(['.', ',', ';']);
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        let recognized_host = reqwest::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_lowercase))
            .is_some_and(|host| {
                MEET_DOMAINS
                    .iter()
                    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
            });
        if recognized_host {
            return Some(url.to_string());
        }
    }
    None
}

/// The event's join link: Google Meet (hangoutLink), else the conferenceData
/// video entry point, else a known conferencing URL in the location or
/// description (Zoom invites live there).
fn conference_link(it: &Value) -> Option<String> {
    if let Some(l) = it.get("hangoutLink").and_then(|s| s.as_str()) {
        return Some(l.to_string());
    }
    if let Some(eps) = it
        .get("conferenceData")
        .and_then(|c| c.get("entryPoints"))
        .and_then(|e| e.as_array())
    {
        let video = eps
            .iter()
            .find(|e| e.get("entryPointType").and_then(|t| t.as_str()) == Some("video"));
        if let Some(uri) = video
            .or_else(|| eps.first())
            .and_then(|e| e.get("uri"))
            .and_then(|u| u.as_str())
        {
            if uri.starts_with("http") {
                return Some(uri.to_string());
            }
        }
    }
    for field in ["location", "description"] {
        if let Some(found) = it
            .get(field)
            .and_then(|s| s.as_str())
            .and_then(find_meeting_url)
        {
            return Some(found);
        }
    }
    None
}
/// One Google event → the Calendar view's shape. Timed events carry `date`
/// (start day, configured zone) plus `start_min`/`end_min` in minutes from that day's
/// midnight — `end_min` exceeds 1440 when the event crosses midnight. All-day
/// events instead carry `end_date`, Google's EXCLUSIVE end day. Returns None
/// for cancelled ghosts.
fn map_event(it: &Value, cal: &GcalCalendar, account: &str) -> Option<Value> {
    if it.get("status").and_then(|s| s.as_str()) == Some("cancelled") {
        return None;
    }
    let title = it
        .get("summary")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(untitled)");
    // Guests, minus rooms/resources: display name (or email) + RSVP status.
    // The visible list stays capped for IPC, but routing gets a separate,
    // uncapped identity list so the thirteenth attendee cannot change where a
    // meeting is filed.
    let attendee_emails: Vec<String> = it
        .get("attendees")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|a| !a.get("resource").and_then(|r| r.as_bool()).unwrap_or(false))
                .filter_map(|a| a.get("email").and_then(|e| e.as_str()))
                .map(|email| email.trim().to_lowercase())
                .filter(|email| !email.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let organizer_email = it
        .get("organizer")
        .and_then(|o| o.get("email"))
        .and_then(|e| e.as_str())
        .map(|email| email.trim().to_lowercase())
        .filter(|email| !email.is_empty());
    let creator_email = it
        .get("creator")
        .and_then(|o| o.get("email"))
        .and_then(|e| e.as_str())
        .map(|email| email.trim().to_lowercase())
        .filter(|email| !email.is_empty());
    let mut associated_emails = Vec::new();
    let mut seen_emails = HashSet::new();
    for email in std::iter::once(account.trim().to_lowercase())
        .chain(organizer_email.iter().cloned())
        .chain(creator_email.iter().cloned())
        .chain(attendee_emails.iter().cloned())
    {
        if !email.is_empty() && seen_emails.insert(email.clone()) {
            associated_emails.push(email);
        }
    }
    let attendee_count = it
        .get("attendees")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter(|x| !x.get("resource").and_then(|r| r.as_bool()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    let attendees: Vec<Value> = it
        .get("attendees")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|a| !a.get("resource").and_then(|r| r.as_bool()).unwrap_or(false))
                .take(12)
                .filter_map(|a| {
                    let name = a
                        .get("displayName")
                        .or_else(|| a.get("email"))
                        .and_then(|s| s.as_str())?;
                    Some(json!({
                        "name": name,
                        "email": a.get("email").and_then(|s| s.as_str()).unwrap_or(""),
                        "status": a.get("responseStatus").and_then(|s| s.as_str()).unwrap_or("needsAction"),
                        "self": a.get("self").and_then(|s| s.as_bool()).unwrap_or(false),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut ev = json!({
        "id": it.get("id").and_then(|s| s.as_str()).unwrap_or_default(),
        "title": title,
        "calendar": cal.name,
        "calendar_id": cal.id,
        "color": cal.color,
        "account": account,
        "location": it.get("location").and_then(|s| s.as_str()),
        // Descriptions can be huge HTML blobs; cap what rides over IPC.
        "description": it
            .get("description")
            .and_then(|s| s.as_str())
            .map(|s| s.chars().take(600).collect::<String>()),
        "declined": is_declined(it),
        "meet_link": conference_link(it),
        // True when the link is Google-managed conference data (so edit can
        // keep/remove it) rather than a Zoom/Teams URL found in free text.
        "google_meet": it.get("hangoutLink").is_some() || it.get("conferenceData").is_some(),
        "html_link": it.get("htmlLink").and_then(|s| s.as_str()),
        "organizer": it.get("organizer").and_then(|o| {
            o.get("displayName").or_else(|| o.get("email")).and_then(|s| s.as_str())
        }),
        "organizer_email": organizer_email,
        "creator_email": creator_email,
        "attendee_emails": attendee_emails,
        "associated_emails": associated_emails,
        "ical_uid": it.get("iCalUID").and_then(|s| s.as_str()),
        // Unlike iCalUID, recurringEventId is present only on an expanded
        // instance of a recurring series. Meeting titles use it to add a date
        // automatically without cluttering one-off event names.
        "recurring_event_id": it.get("recurringEventId").and_then(|s| s.as_str()),
        "attendees": attendees,
        "attendee_count": attendee_count,
    });
    match it
        .get("start")
        .and_then(|s| s.get("dateTime"))
        .and_then(|s| s.as_str())
    {
        Some(sdt) => {
            let time_zone = crate::system_settings::time_zone();
            let s = chrono::DateTime::parse_from_rfc3339(sdt)
                .ok()?
                .with_timezone(&time_zone);
            let start_min = (s.hour() * 60 + s.minute()) as i64;
            let end_min = it
                .get("end")
                .and_then(|e| e.get("dateTime"))
                .and_then(|e| e.as_str())
                .and_then(|edt| chrono::DateTime::parse_from_rfc3339(edt).ok())
                .map(|e| {
                    let e = e.with_timezone(&time_zone);
                    let days = (e.date_naive() - s.date_naive()).num_days();
                    days * 1440 + (e.hour() * 60 + e.minute()) as i64
                })
                .unwrap_or(start_min + 60)
                // Zero/negative spans still get a visible, clickable block.
                .max(start_min + 15);
            ev["date"] = json!(s.format("%Y-%m-%d").to_string());
            ev["end_date"] = Value::Null;
            ev["start_min"] = json!(start_min);
            ev["end_min"] = json!(end_min);
            ev["all_day"] = json!(false);
        }
        None => {
            let date = it
                .get("start")
                .and_then(|s| s.get("date"))
                .and_then(|s| s.as_str())?;
            let end_date = it
                .get("end")
                .and_then(|e| e.get("date"))
                .and_then(|e| e.as_str())
                .unwrap_or(date);
            ev["date"] = json!(date);
            ev["end_date"] = json!(end_date);
            ev["start_min"] = Value::Null;
            ev["end_min"] = Value::Null;
            ev["all_day"] = json!(true);
        }
    }
    Some(ev)
}

async fn fetch_calendar_events(
    client: &reqwest::Client,
    token: &str,
    cal: &GcalCalendar,
    account: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<Value>> {
    let url = format!(
        "{CAL_BASE}/calendars/{}/events?singleEvents=true&orderBy=startTime&timeMin={}&timeMax={}&maxResults=250&showDeleted=false",
        enc(&cal.id),
        enc(time_min),
        enc(time_max),
    );
    let resp = client.get(url).bearer_auth(token).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "events for {} failed ({})",
            cal.name,
            resp.status()
        ));
    }
    let v: Value = resp.json().await?;
    Ok(v.get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| map_event(it, cal, account))
                .collect()
        })
        .unwrap_or_default())
}

/// Every enabled calendar's events across every connected account for the
/// inclusive day range [start_date, end_date] — the Calendar view's feed.
/// Per-calendar fetches fan out concurrently; a broken account session or an
/// unreadable calendar is skipped rather than blanking the whole grid.
pub async fn events_range(dir: &Path, start_date: &str, end_date: &str) -> Result<Vec<Value>> {
    let s = NaiveDate::parse_from_str(start_date, "%Y-%m-%d")?;
    let e = NaiveDate::parse_from_str(end_date, "%Y-%m-%d")?;
    let days = (e - s).num_days() + 1;
    if !(1..=62).contains(&days) {
        return Err(anyhow!("bad range {start_date}..{end_date}"));
    }
    let time_min = rfc3339_from_minutes(start_date, 0)?;
    let time_max = rfc3339_from_minutes(start_date, days * 1440)?;

    let emails: Vec<String> = get()
        .accounts
        .iter()
        .filter(|a| a.refresh_token.is_some())
        .map(|a| a.email.clone())
        .collect();
    if emails.is_empty() {
        return Err(anyhow!("not connected to Google Calendar"));
    }

    let client = http_client()?;
    let mut set = tokio::task::JoinSet::new();
    for email in emails {
        // Tokens refresh serially (once per account); event reads fan out.
        let Ok(token) = access_token_for(dir, &email).await else {
            continue;
        };
        let cals = ensure_calendars(dir, &email).await.unwrap_or_default();
        for cal in cals.into_iter().filter(|c| c.enabled) {
            let (client, token, email) = (client.clone(), token.clone(), email.clone());
            let (time_min, time_max) = (time_min.clone(), time_max.clone());
            set.spawn(async move {
                fetch_calendar_events(&client, &token, &cal, &email, &time_min, &time_max).await
            });
        }
    }

    let mut events: Vec<Value> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Ok(batch)) = res {
            events.extend(batch);
        }
    }
    // Day order; all-day first within a day (they render in the banner row).
    events.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v.get("date")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                v.get("start_min").and_then(|m| m.as_i64()).unwrap_or(-1),
            )
        };
        key(a).cmp(&key(b))
    });
    harvest_contacts(dir, &events);
    Ok(events)
}

/// Remember attendee emails seen on events — the event form's guest
/// autocomplete pool. Local-first by design: no Google contacts scope, just
/// the people the user actually meets with, accumulated as ranges load.
fn harvest_contacts(dir: &Path, events: &[Value]) {
    let mut changed = false;
    {
        let mut c = cell().write().unwrap();
        for ev in events {
            let Some(atts) = ev.get("attendees").and_then(|a| a.as_array()) else {
                continue;
            };
            for a in atts {
                if a.get("self").and_then(|s| s.as_bool()).unwrap_or(false) {
                    continue; // no point autocompleting yourself
                }
                let Some(email) = a.get("email").and_then(|e| e.as_str()) else {
                    continue;
                };
                let email = email.trim().to_lowercase();
                if email.is_empty() || !email.contains('@') {
                    continue;
                }
                let name = a
                    .get("name")
                    .and_then(|n| n.as_str())
                    .filter(|n| !n.eq_ignore_ascii_case(&email))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if let Some(k) = c.contacts.iter_mut().find(|k| k.email == email) {
                    if k.name.is_empty() && !name.is_empty() {
                        k.name = name;
                        changed = true;
                    }
                } else if c.contacts.len() < 500 {
                    c.contacts.push(GcalContact { email, name });
                    changed = true;
                }
            }
        }
    }
    if changed {
        write_config_file(dir);
    }
}

// ── Event CRUD (Calendar view) ──────────────────────────────────────────────
/// Google error bodies are verbose nested JSON; surface just the human
/// sentence ("Invalid attendee email.") and keep the raw body as a fallback.
fn api_error_detail(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| text.to_string())
}
/// Request body for a user-authored event. `start`/`end` are "HH:MM"; an
/// absent start means all-day, with `end_date` the INCLUSIVE last day
/// (defaults to `date`). With `patch`, the unused date/dateTime representation
/// is explicitly nulled — PATCH keeps whatever it isn't told to clear, and
/// Google rejects an event carrying both.
/// `meet`: Some(true) asks Google to attach a Meet conference, Some(false)
/// strips the existing one, None leaves conference data untouched. Requests
/// carrying it must go to a `conferenceDataVersion=1` URL or Google ignores it.
#[allow(clippy::too_many_arguments)]
fn user_event_body(
    title: &str,
    date: &str,
    start: Option<&str>,
    end: Option<&str>,
    end_date: Option<&str>,
    location: Option<&str>,
    description: Option<&str>,
    meet: Option<bool>,
    patch: bool,
) -> Result<Value> {
    let title = title.trim();
    if title.is_empty() {
        return Err(anyhow!("the event needs a title"));
    }
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| anyhow!("bad date {date}"))?;
    let mut body = json!({ "summary": title });
    match start.map(str::trim).filter(|s| !s.is_empty()) {
        Some(start) => {
            let smin = parse_hhmm(start)?;
            let emin = match end.map(str::trim).filter(|s| !s.is_empty()) {
                Some(end) => {
                    let mut m = parse_hhmm(end)?;
                    if m <= smin {
                        m += 1440; // crosses midnight
                    }
                    m
                }
                None => smin + 60,
            };
            body["start"] = json!({ "dateTime": rfc3339_from_minutes(date, smin)? });
            body["end"] = json!({ "dateTime": rfc3339_from_minutes(date, emin)? });
        }
        None => {
            let start_d = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
            let last = match end_date.map(str::trim).filter(|s| !s.is_empty()) {
                Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .map_err(|_| anyhow!("bad date {d}"))?
                    .max(start_d),
                None => start_d,
            };
            let excl = last + chrono::Duration::days(1);
            body["start"] = json!({ "date": date });
            body["end"] = json!({ "date": excl.format("%Y-%m-%d").to_string() });
        }
    }
    if patch {
        for k in ["start", "end"] {
            for f in ["dateTime", "date"] {
                if body[k].get(f).is_none() {
                    body[k][f] = Value::Null;
                }
            }
        }
    }
    if let Some(l) = location {
        body["location"] = json!(l.trim());
    }
    if let Some(d) = description {
        body["description"] = json!(d);
    }
    match meet {
        Some(true) => {
            body["conferenceData"] = json!({
                "createRequest": {
                    "requestId": rand_token(),
                    "conferenceSolutionKey": { "type": "hangoutsMeet" },
                }
            });
        }
        Some(false) => body["conferenceData"] = Value::Null,
        None => {}
    }
    Ok(body)
}

/// Create an event on any connected account's calendar. Returns `{ id }`.
/// `add_meet` attaches a Google Meet conference; `guests` are attendee emails
/// (they get invite emails, matching what Google Calendar's own UI does).
#[allow(clippy::too_many_arguments)]
pub async fn create_event(
    dir: &Path,
    account: &str,
    calendar_id: &str,
    title: &str,
    date: &str,
    start: Option<&str>,
    end: Option<&str>,
    end_date: Option<&str>,
    location: Option<&str>,
    description: Option<&str>,
    add_meet: bool,
    guests: &[String],
) -> Result<Value> {
    let token = access_token_for(dir, account).await?;
    let meet = if add_meet { Some(true) } else { None };
    let mut body = user_event_body(
        title,
        date,
        start,
        end,
        end_date,
        location,
        description,
        meet,
        false,
    )?;
    let guests: Vec<&str> = guests
        .iter()
        .map(|g| g.trim())
        .filter(|g| g.contains('@'))
        .collect();
    if !guests.is_empty() {
        body["attendees"] = json!(guests
            .iter()
            .map(|g| json!({ "email": g }))
            .collect::<Vec<_>>());
    }
    // conferenceDataVersion=1 is required for Meet creation (harmless without);
    // sendUpdates emails the invite only when there are guests to invite.
    let send = if guests.is_empty() { "none" } else { "all" };
    let resp = http_client()?
        .post(format!(
            "{CAL_BASE}/calendars/{}/events?conferenceDataVersion=1&sendUpdates={send}",
            enc(calendar_id)
        ))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "create failed ({status}): {}",
            api_error_detail(&text)
        ));
    }
    let v: Value = resp.json().await?;
    Ok(json!({ "id": v.get("id").cloned().unwrap_or(Value::Null) }))
}

/// Edit an event in place. `move_to` first relocates it to another calendar in
/// the SAME account (Google can't move events across accounts). `meet`:
/// Some(true) attaches a Google Meet, Some(false) removes it, None keeps
/// whatever is there.
#[allow(clippy::too_many_arguments)]
pub async fn update_event(
    dir: &Path,
    account: &str,
    calendar_id: &str,
    event_id: &str,
    title: &str,
    date: &str,
    start: Option<&str>,
    end: Option<&str>,
    end_date: Option<&str>,
    location: Option<&str>,
    description: Option<&str>,
    move_to: Option<&str>,
    meet: Option<bool>,
) -> Result<()> {
    let token = access_token_for(dir, account).await?;
    let client = http_client()?;
    let mut cal = calendar_id.to_string();
    if let Some(dest) = move_to.filter(|d| !d.is_empty() && *d != calendar_id) {
        let resp = client
            .post(format!(
                "{CAL_BASE}/calendars/{}/events/{}/move?destination={}",
                enc(&cal),
                enc(event_id),
                enc(dest),
            ))
            .bearer_auth(&token)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "move failed ({status}): {}",
                api_error_detail(&text)
            ));
        }
        cal = dest.to_string();
    }
    let body = user_event_body(
        title,
        date,
        start,
        end,
        end_date,
        location,
        description,
        meet,
        true,
    )?;
    let resp = client
        .patch(format!(
            "{CAL_BASE}/calendars/{}/events/{}?conferenceDataVersion=1",
            enc(&cal),
            enc(event_id)
        ))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "update failed ({status}): {}",
            api_error_detail(&text)
        ));
    }
    Ok(())
}

/// Delete an event from any connected account's calendar (already-gone is fine).
pub async fn remove_event(
    dir: &Path,
    account: &str,
    calendar_id: &str,
    event_id: &str,
) -> Result<()> {
    let token = access_token_for(dir, account).await?;
    delete_event(&http_client()?, &token, calendar_id, event_id).await
}

// ── Read-back (Today empty state) ─────────────────────────────────────────────
/// Convert an RFC3339 instant to wall-clock "HH:MM" in the configured zone.
fn hhmm_from_rfc3339(s: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    let time_zone = crate::system_settings::time_zone();
    Some(dt.with_timezone(&time_zone).format("%H:%M").to_string())
}

/// True if the signed-in user has declined this event (so we hide it).
fn is_declined(it: &Value) -> bool {
    it.get("attendees")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter().any(|a| {
                a.get("self").and_then(|s| s.as_bool()).unwrap_or(false)
                    && a.get("responseStatus").and_then(|s| s.as_str()) == Some("declined")
            })
        })
        .unwrap_or(false)
}

/// Pull every (non-noted) calendar's events for `event_date` so the Today view
/// can show what the user already has planned. Read-only; merges across EVERY
/// connected account's enabled calendars, hides events the user declined, and
/// skips noted's own pushed calendar. All-day events come back with
/// `start: null, all_day: true`.
/// Each item also carries stable event/account identity and its join link so
/// Today can resolve a schedule row without persisting stale URL metadata.
pub async fn list_events(dir: &Path, event_date: &str) -> Result<Vec<Value>> {
    // Day window in the configured wall clock — matches stored/displayed blocks.
    let time_min = rfc3339_from_minutes(event_date, 0)?;
    let time_max = rfc3339_from_minutes(event_date, 1440)?;

    let emails: Vec<String> = get()
        .accounts
        .iter()
        .filter(|a| a.refresh_token.is_some())
        .map(|a| a.email.clone())
        .collect();
    if emails.is_empty() {
        return Err(anyhow!("not connected to Google Calendar"));
    }

    let client = http_client()?;
    let mut events: Vec<Value> = Vec::new();
    for email in &emails {
        // One dead account session shouldn't blank the whole day.
        let token = match access_token_for(dir, email).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[noted] gcal token for {email} failed: {e}");
                continue;
            }
        };
        let cals = ensure_calendars(dir, email).await.unwrap_or_default();
        for cal in cals.iter().filter(|c| c.enabled && c.name != CAL_SUMMARY) {
            let url = format!(
                "{CAL_BASE}/calendars/{}/events?singleEvents=true&orderBy=startTime&timeMin={}&timeMax={}&maxResults=50&showDeleted=false",
                enc(&cal.id),
                enc(&time_min),
                enc(&time_max),
            );
            let resp = client.get(url).bearer_auth(&token).send().await?;
            // Skip calendars we can't read rather than failing the whole pull.
            if !resp.status().is_success() {
                eprintln!(
                    "[noted] gcal events for {} failed ({})",
                    cal.name,
                    resp.status()
                );
                continue;
            }
            let v: Value = resp.json().await?;
            let Some(items) = v.get("items").and_then(|i| i.as_array()) else {
                continue;
            };
            for it in items {
                if is_declined(it) {
                    continue;
                }
                let task = it
                    .get("summary")
                    .and_then(|s| s.as_str())
                    .unwrap_or("(busy)")
                    .trim();
                // Timed events carry start.dateTime; all-day events carry start.date.
                let (start_hhmm, all_day) = match it
                    .get("start")
                    .and_then(|s| s.get("dateTime"))
                    .and_then(|s| s.as_str())
                {
                    Some(dt) => (hhmm_from_rfc3339(dt), false),
                    None => (None, true),
                };
                let end_hhmm = it
                    .get("end")
                    .and_then(|s| s.get("dateTime"))
                    .and_then(|s| s.as_str())
                    .and_then(hhmm_from_rfc3339);
                events.push(json!({
                    "id": it.get("id").and_then(|v| v.as_str()).unwrap_or_default(),
                    "task": task,
                    "start": start_hhmm,
                    "end": end_hhmm,
                    "all_day": all_day,
                    "calendar": cal.name,
                    "calendar_id": cal.id,
                    "color": cal.color,
                    "account": email,
                    "meet_link": conference_link(it),
                    "html_link": it.get("htmlLink").and_then(|v| v.as_str()),
                }));
            }
        }
    }

    // Chronological; all-day / untimed events sort to the end (stable for ties).
    events.sort_by_key(|e| {
        e.get("start")
            .and_then(|s| s.as_str())
            .and_then(|s| parse_hhmm(s).ok())
            .unwrap_or(i64::MAX)
    });
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_accounts_remain_owner_identities_without_live_tokens() {
        let account = |email: &str, refresh_token: Option<&str>| GcalAccount {
            email: email.into(),
            calendars: Vec::new(),
            refresh_token: refresh_token.map(str::to_string),
            access_token: None,
            access_expires_at: None,
        };
        let config = GcalConfig {
            accounts: vec![
                account(" Edison@HeyBorrow.com ", None),
                account("personal@gmail.com", Some("token")),
                account("edison@heyborrow.com", Some("new-token")),
            ],
            ..GcalConfig::default()
        };

        assert_eq!(
            configured_account_emails_from(&config),
            vec!["edison@heyborrow.com", "personal@gmail.com"]
        );
    }

    #[test]
    fn event_id_is_deterministic_and_valid() {
        let a = event_id("2026-06-04", "gym", "09:00");
        let b = event_id("2026-06-04", "gym", "09:00");
        assert_eq!(a, b, "same date+task+start must yield the same id");
        assert_ne!(a, event_id("2026-06-04", "lunch", "09:00"));
        assert_ne!(a, event_id("2026-06-04", "gym", "10:00"));
        assert_ne!(a, event_id("2026-06-05", "gym", "09:00"));
        // Whitespace-insensitive on task so a stray trim doesn't orphan events.
        assert_eq!(a, event_id("2026-06-04", "  gym  ", "09:00"));
        // base32hex charset: a-v and 0-9 only, length within Google's 5..=1024.
        assert!(a.len() >= 5 && a.len() <= 1024);
        assert!(
            a.chars().all(|c| matches!(c, 'a'..='v' | '0'..='9')),
            "id {a} must be base32hex"
        );
    }

    #[test]
    fn event_id_is_independent_of_sibling_blocks() {
        // The corruption this guards against: with index-keyed ids, deleting
        // block 0 shifted every later block onto its neighbor's event. Content
        // keying makes each block's id a pure function of the block itself.
        let lunch_alone = event_id("2026-06-04", "lunch", "12:00");
        let lunch_after_gym = event_id("2026-06-04", "lunch", "12:00");
        assert_eq!(lunch_alone, lunch_after_gym);
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
        assert!(ev["start"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-04T09:00:00"));
        assert!(ev["end"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-04T10:00:00"));
        assert_eq!(
            ev["extendedProperties"]["private"]["notedDate"],
            "2026-06-04"
        );
    }

    #[test]
    fn build_event_crosses_midnight_when_end_before_start() {
        let block = json!({ "task": "sleep", "start": "23:00", "end": "06:00" });
        let ev = build_event("2026-06-04", "sleep", "23:00", &block, "notedxyz").unwrap();
        assert!(ev["start"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-04T23:00:00"));
        assert!(ev["end"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-05T06:00:00"));
    }

    #[test]
    fn hhmm_from_minutes_formats_and_wraps() {
        assert_eq!(hhmm_from_minutes(9 * 60), "09:00");
        assert_eq!(hhmm_from_minutes(14 * 60 + 30), "14:30");
        // Cross-midnight end (e.g. 06:00 stored as 1800 min) wraps to wall-clock.
        assert_eq!(hhmm_from_minutes(30 * 60), "06:00");
    }

    #[test]
    fn block_end_minutes_matches_build_event_logic() {
        // Explicit end.
        let b = json!({ "start": "09:00", "end": "10:30" });
        assert_eq!(block_end_minutes(9 * 60, &b).unwrap(), 10 * 60 + 30);
        // duration_min.
        let b = json!({ "start": "09:00", "duration_min": 45 });
        assert_eq!(block_end_minutes(9 * 60, &b).unwrap(), 9 * 60 + 45);
        // Default one hour.
        let b = json!({ "start": "09:00" });
        assert_eq!(block_end_minutes(9 * 60, &b).unwrap(), 10 * 60);
        // End ≤ start rolls past midnight.
        let b = json!({ "start": "23:00", "end": "06:00" });
        assert_eq!(block_end_minutes(23 * 60, &b).unwrap(), 30 * 60);
    }

    #[test]
    fn hhmm_from_rfc3339_converts_to_eastern() {
        // 13:30 UTC in June → 09:30 EDT
        assert_eq!(
            hhmm_from_rfc3339("2026-06-05T13:30:00Z").as_deref(),
            Some("09:30")
        );
        // Already in the configured zone → passes through unchanged in the default test setup.
        assert_eq!(
            hhmm_from_rfc3339("2026-06-05T09:30:00-04:00").as_deref(),
            Some("09:30")
        );
        assert_eq!(hhmm_from_rfc3339("garbage"), None);
    }

    fn cal(id: &str) -> GcalCalendar {
        GcalCalendar {
            id: id.into(),
            name: id.into(),
            color: "#039be5".into(),
            enabled: true,
            primary: false,
            access: "owner".into(),
        }
    }

    #[test]
    fn merge_calendars_keeps_hidden_choices() {
        let mut old_work = cal("work");
        old_work.enabled = false;
        let old = vec![old_work, cal("home"), cal("gone")];
        // Fresh list: "gone" was deleted upstream, "new" appeared, colors refresh.
        let merged = merge_calendars(old, vec![cal("work"), cal("home"), cal("new")]);
        let by_id = |id: &str| merged.iter().find(|c| c.id == id);
        assert!(
            !by_id("work").unwrap().enabled,
            "hide choice must survive refresh"
        );
        assert!(by_id("home").unwrap().enabled);
        assert!(by_id("new").unwrap().enabled, "new calendars start visible");
        assert!(by_id("gone").is_none(), "deleted calendars drop out");
    }

    #[test]
    fn map_event_timed_converts_to_eastern_minutes() {
        let it = json!({
            "id": "abc",
            "summary": "standup",
            "start": { "dateTime": "2026-06-05T13:30:00Z" }, // 09:30 EDT
            "end": { "dateTime": "2026-06-05T14:00:00Z" },
            "location": "zoom",
        });
        let ev = map_event(&it, &cal("work"), "me@x.com").unwrap();
        assert_eq!(ev["date"], "2026-06-05");
        assert_eq!(ev["start_min"], 9 * 60 + 30);
        assert_eq!(ev["end_min"], 10 * 60);
        assert_eq!(ev["all_day"], false);
        assert_eq!(ev["account"], "me@x.com");
        assert_eq!(ev["calendar"], "work");
        assert_eq!(ev["location"], "zoom");
    }

    #[test]
    fn map_event_cross_midnight_end_exceeds_1440() {
        let it = json!({
            "summary": "redeye",
            "start": { "dateTime": "2026-06-05T23:00:00-04:00" },
            "end": { "dateTime": "2026-06-06T01:00:00-04:00" },
        });
        let ev = map_event(&it, &cal("c"), "a").unwrap();
        assert_eq!(ev["start_min"], 23 * 60);
        assert_eq!(ev["end_min"], 25 * 60);
    }

    #[test]
    fn map_event_all_day_keeps_exclusive_end_date() {
        let it = json!({
            "summary": "conference",
            "start": { "date": "2026-06-05" },
            "end": { "date": "2026-06-08" }, // Google end is exclusive: 3 days
        });
        let ev = map_event(&it, &cal("c"), "a").unwrap();
        assert_eq!(ev["all_day"], true);
        assert_eq!(ev["date"], "2026-06-05");
        assert_eq!(ev["end_date"], "2026-06-08");
        assert_eq!(ev["start_min"], Value::Null);
    }

    #[test]
    fn resolve_sync_account_prefers_choice_then_first() {
        let accounts = vec![
            GcalAccount {
                email: "a@x.com".into(),
                ..Default::default()
            },
            GcalAccount {
                email: "b@y.com".into(),
                ..Default::default()
            },
        ];
        // No preference → first account.
        assert_eq!(
            resolve_sync_account(&accounts, None).as_deref(),
            Some("a@x.com")
        );
        // Explicit choice wins.
        assert_eq!(
            resolve_sync_account(&accounts, Some("b@y.com")).as_deref(),
            Some("b@y.com")
        );
        // A removed account's stale preference falls back to first.
        assert_eq!(
            resolve_sync_account(&accounts, Some("gone@z.com")).as_deref(),
            Some("a@x.com")
        );
        assert_eq!(resolve_sync_account(&[], None), None);
    }

    #[test]
    fn conference_link_prefers_hangout_then_conference_then_text() {
        // hangoutLink wins outright.
        let ev = json!({ "hangoutLink": "https://meet.google.com/abc-defg-hij" });
        assert_eq!(
            conference_link(&ev).as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        // conferenceData video entry point.
        let ev = json!({ "conferenceData": { "entryPoints": [
            { "entryPointType": "phone", "uri": "tel:+1-555-0100" },
            { "entryPointType": "video", "uri": "https://meet.google.com/xyz" },
        ]}});
        assert_eq!(
            conference_link(&ev).as_deref(),
            Some("https://meet.google.com/xyz")
        );
        // Zoom link parked in the location.
        let ev = json!({ "location": "https://company.zoom.us/j/123?pwd=x" });
        assert_eq!(
            conference_link(&ev).as_deref(),
            Some("https://company.zoom.us/j/123?pwd=x")
        );
        // Zoom link inside an HTML description href.
        let ev = json!({ "description": "Agenda…<a href=\"https://zoom.us/j/9\">Join</a>" });
        assert_eq!(conference_link(&ev).as_deref(), Some("https://zoom.us/j/9"));
        // Discord calls are meetings too, whether the invite uses the full or short host.
        let ev = json!({ "location": "https://discord.com/channels/123/456" });
        assert_eq!(
            conference_link(&ev).as_deref(),
            Some("https://discord.com/channels/123/456")
        );
        let ev = json!({ "description": "Join at https://discord.gg/noted" });
        assert_eq!(
            conference_link(&ev).as_deref(),
            Some("https://discord.gg/noted")
        );
        // A trusted host appearing only in a query string must not pass.
        let ev = json!({ "description": "https://evil.example/?next=discord.gg/noted" });
        assert_eq!(conference_link(&ev), None);
        // Ordinary URLs don't count as meetings.
        let ev = json!({ "description": "notes at https://example.com/doc" });
        assert_eq!(conference_link(&ev), None);
    }

    #[test]
    fn map_event_carries_meeting_details() {
        let it = json!({
            "summary": "standup",
            "start": { "dateTime": "2026-06-05T13:30:00Z" },
            "end": { "dateTime": "2026-06-05T14:00:00Z" },
            "hangoutLink": "https://meet.google.com/abc",
            "htmlLink": "https://calendar.google.com/event?eid=123",
            "organizer": { "email": "khai@x.com", "displayName": "Khai" },
            "creator": { "email": "assistant@x.com", "displayName": "Assistant" },
            "iCalUID": "event-123@google.com",
            "recurringEventId": "event-123",
            "attendees": [
                { "email": "khai@x.com", "displayName": "Khai", "responseStatus": "accepted" },
                { "email": "me@x.com", "self": true, "responseStatus": "needsAction" },
                { "email": "room-4@resource.calendar.google.com", "resource": true },
            ],
        });
        let ev = map_event(&it, &cal("work"), "me@x.com").unwrap();
        assert_eq!(ev["meet_link"], "https://meet.google.com/abc");
        assert_eq!(ev["html_link"], "https://calendar.google.com/event?eid=123");
        assert_eq!(ev["organizer"], "Khai");
        assert_eq!(ev["organizer_email"], "khai@x.com");
        assert_eq!(ev["creator_email"], "assistant@x.com");
        assert_eq!(ev["ical_uid"], "event-123@google.com");
        assert_eq!(ev["recurring_event_id"], "event-123");
        assert_eq!(
            ev["associated_emails"],
            json!(["me@x.com", "khai@x.com", "assistant@x.com"])
        );
        // The meeting room resource is filtered out of both list and count.
        assert_eq!(ev["attendee_count"], 2);
        assert_eq!(ev["attendees"].as_array().unwrap().len(), 2);
        assert_eq!(ev["attendees"][0]["name"], "Khai");
        assert_eq!(ev["attendees"][1]["self"], true);
    }

    #[test]
    fn map_event_keeps_uncapped_identity_emails_for_routing() {
        let attendees: Vec<Value> = (0..15)
            .map(|index| json!({ "email": format!("person{index}@example.com") }))
            .collect();
        let it = json!({
            "summary": "Large meeting",
            "start": { "dateTime": "2026-06-05T13:30:00Z" },
            "end": { "dateTime": "2026-06-05T14:00:00Z" },
            "attendees": attendees,
        });
        let ev = map_event(&it, &cal("work"), "owner@example.com").unwrap();
        assert_eq!(ev["attendees"].as_array().unwrap().len(), 12);
        assert_eq!(ev["attendee_emails"].as_array().unwrap().len(), 15);
        assert_eq!(ev["associated_emails"].as_array().unwrap().len(), 16);
        assert!(ev["associated_emails"]
            .as_array()
            .unwrap()
            .iter()
            .any(|email| email == "person14@example.com"));
    }

    #[test]
    fn map_event_skips_cancelled_and_titles_untitled() {
        let gone = json!({ "status": "cancelled", "start": { "date": "2026-06-05" } });
        assert!(map_event(&gone, &cal("c"), "a").is_none());
        let untitled =
            json!({ "start": { "date": "2026-06-05" }, "end": { "date": "2026-06-06" } });
        assert_eq!(
            map_event(&untitled, &cal("c"), "a").unwrap()["title"],
            "(untitled)"
        );
    }

    #[test]
    fn user_event_body_timed_and_all_day() {
        let b = user_event_body(
            "lunch",
            "2026-06-05",
            Some("12:00"),
            Some("13:00"),
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(b["start"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-05T12:00:00"));
        assert!(b["end"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-05T13:00:00"));
        assert!(
            b["start"].get("date").is_none(),
            "no null padding on create"
        );

        // All-day: inclusive last day → Google's exclusive end.
        let b = user_event_body(
            "offsite",
            "2026-06-05",
            None,
            None,
            Some("2026-06-06"),
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(b["start"]["date"], "2026-06-05");
        assert_eq!(b["end"]["date"], "2026-06-07");
    }

    #[test]
    fn user_event_body_patch_nulls_the_other_shape() {
        let b = user_event_body(
            "x",
            "2026-06-05",
            Some("09:00"),
            None,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            b["start"]["date"],
            Value::Null,
            "patch must clear the all-day shape"
        );
        assert!(b["start"]["dateTime"].is_string());
        // Default end: one hour.
        assert!(b["end"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-05T10:00:00"));

        let b =
            user_event_body("x", "2026-06-05", None, None, None, None, None, None, true).unwrap();
        assert_eq!(
            b["start"]["dateTime"],
            Value::Null,
            "patch must clear the timed shape"
        );
        assert_eq!(b["end"]["date"], "2026-06-06");
    }

    #[test]
    fn api_error_detail_extracts_the_human_message() {
        // The exact shape Google returned for a malformed guest email.
        let body = r#"{ "error": { "errors": [ { "domain": "global", "reason": "invalid", "message": "Invalid attendee email." } ], "code": 400, "message": "Invalid attendee email." } }"#;
        assert_eq!(api_error_detail(body), "Invalid attendee email.");
        // Non-JSON bodies pass through untouched.
        assert_eq!(api_error_detail("plain text"), "plain text");
    }

    #[test]
    fn user_event_body_meet_tristate() {
        // Some(true): ask Google to mint a Meet conference.
        let b = user_event_body(
            "sync",
            "2026-06-05",
            Some("09:00"),
            None,
            None,
            None,
            None,
            Some(true),
            false,
        )
        .unwrap();
        assert_eq!(
            b["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"],
            "hangoutsMeet"
        );
        assert!(
            !b["conferenceData"]["createRequest"]["requestId"]
                .as_str()
                .unwrap()
                .is_empty(),
            "createRequest needs a client-generated requestId"
        );
        // Some(false): strip the existing conference on PATCH.
        let b = user_event_body(
            "sync",
            "2026-06-05",
            Some("09:00"),
            None,
            None,
            None,
            None,
            Some(false),
            true,
        )
        .unwrap();
        assert_eq!(b["conferenceData"], Value::Null);
        // None: conference data untouched.
        let b = user_event_body(
            "sync",
            "2026-06-05",
            Some("09:00"),
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(b.get("conferenceData").is_none());
    }

    #[test]
    fn user_event_body_rolls_end_past_midnight_and_rejects_empty_title() {
        let b = user_event_body(
            "late",
            "2026-06-05",
            Some("23:00"),
            Some("01:00"),
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(b["end"]["dateTime"]
            .as_str()
            .unwrap()
            .starts_with("2026-06-06T01:00:00"));
        assert!(user_event_body(
            "  ",
            "2026-06-05",
            None,
            None,
            None,
            None,
            None,
            None,
            false
        )
        .is_err());
    }

    #[test]
    fn is_declined_only_for_self_declined() {
        let declined = json!({ "attendees": [{ "self": true, "responseStatus": "declined" }] });
        assert!(is_declined(&declined));
        let accepted = json!({ "attendees": [{ "self": true, "responseStatus": "accepted" }] });
        assert!(!is_declined(&accepted));
        // Someone else declined, not us.
        let other = json!({ "attendees": [{ "self": false, "responseStatus": "declined" }] });
        assert!(!is_declined(&other));
        assert!(!is_declined(&json!({ "summary": "solo" })));
    }
}
