//! Apple Calendar (EventKit) — read-only.
//!
//! Unlike `gcal`, this needs no OAuth, no refresh tokens in the Keychain, and no
//! network: EventKit reads the calendar database this Mac already syncs, which
//! fits the local-first design better than a cloud round-trip.
//!
//! EventKit is called **in process** on purpose. TCC attributes a calendar
//! prompt to the binary that asks, so asking from here keeps the prompt as
//! noted itself; a spawned helper would name the helper.
//!
//! Scope is deliberately read-only: events surface in the Calendar view, Today,
//! and meeting detection, but nothing here can modify a real calendar.
//!
//! The read path merges with Google in `calendar::events_range`, so every
//! existing consumer of that function gains Apple events without changes.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{OnceLock, RwLock};

use chrono::{TimeZone, Timelike};

/// Account label for Apple events. `RangeEvent.account` already namespaces by
/// account, so the Calendar view groups these without frontend changes.
pub const ACCOUNT: &str = "Apple Calendar";

// ---------------------------------------------------------------------------
// Config: applecal.json in app data (meetings.json pattern — no secrets).
// ---------------------------------------------------------------------------

/// Stores the calendars the user switched **off** rather than the ones left on:
/// a calendar added in Calendar.app should show up without needing a visit to
/// Settings, and an empty config must mean "show everything", not "show none".
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppleCalCfg {
    #[serde(default)]
    pub hidden_calendars: Vec<String>,
}

fn cfg_cell() -> &'static RwLock<AppleCalCfg> {
    static CFG: OnceLock<RwLock<AppleCalCfg>> = OnceLock::new();
    CFG.get_or_init(|| RwLock::new(AppleCalCfg::default()))
}

pub fn cfg() -> AppleCalCfg {
    cfg_cell().read().unwrap().clone()
}

pub fn cfg_init(dir: &Path) {
    let _ = LOG_DIR.set(dir.to_path_buf());
    if let Ok(text) = std::fs::read_to_string(dir.join("applecal.json")) {
        if let Ok(loaded) = serde_json::from_str::<AppleCalCfg>(&text) {
            *cfg_cell().write().unwrap() = loaded;
        }
    }
}

fn cfg_write(dir: &Path, next: AppleCalCfg) -> Result<()> {
    std::fs::write(
        dir.join("applecal.json"),
        serde_json::to_string_pretty(&next)?,
    )?;
    *cfg_cell().write().unwrap() = next;
    Ok(())
}

/// Show or hide one calendar in the merged feed.
pub fn set_calendar_enabled(dir: &Path, calendar_id: &str, enabled: bool) -> Result<Value> {
    let mut next = cfg();
    next.hidden_calendars.retain(|id| id != calendar_id);
    if !enabled {
        next.hidden_calendars.push(calendar_id.to_string());
    }
    cfg_write(dir, next)?;
    status()
}

// ---------------------------------------------------------------------------
// Access + reads
// ---------------------------------------------------------------------------

/// Append a line to `applecal.log` in the app data dir.
///
/// Calendar access is granted by the OS, not by us, and the interesting
/// failures happen where no console is attached (a double-clicked app has
/// nowhere to send stderr). A file keeps the evidence either way.
fn log_line(message: &str) {
    eprintln!("[noted] applecal: {message}");
    if let Some(dir) = LOG_DIR.get() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("applecal.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "{} {message}", chrono::Local::now().to_rfc3339());
        }
    }
}

static LOG_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Whether this Mac can serve Apple Calendar, and whether the user has allowed
/// it. An enum rather than a bare string so a mistyped comparison cannot
/// silently disable calendar reads; `as_str` is the wire form the frontend
/// switches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Granted,
    Denied,
    Restricted,
    /// EventKit's write-only grant cannot read events, so for a read-only
    /// integration it is no more useful than no access at all — reported
    /// distinctly rather than as an empty calendar that looks like a bug.
    WriteOnly,
    NotDetermined,
    /// Not macOS: there is no Apple Calendar to read.
    Unsupported,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Granted => "granted",
            Access::Denied => "denied",
            Access::Restricted => "restricted",
            Access::WriteOnly => "write_only",
            Access::NotDetermined => "not_determined",
            Access::Unsupported => "unsupported",
        }
    }

    /// The single definition of "we may read events".
    pub fn can_read(self) -> bool {
        self == Access::Granted
    }
}

pub fn access_state() -> Access {
    #[cfg(target_os = "macos")]
    {
        mac::access_state()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Access::Unsupported
    }
}

/// Prompt for calendar access. Returns the resulting access state.
///
/// The OS shows its prompt only the first time; once denied, the user has to
/// change it in System Settings, so a repeat call just reports `denied`.
pub async fn request_access(app: &tauri::AppHandle) -> Result<Access> {
    #[cfg(target_os = "macos")]
    {
        mac::request_access(app).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(Access::Unsupported)
    }
}

/// Calendars EventKit knows about, each flagged with whether it is shown.
pub fn calendars() -> Vec<Value> {
    #[cfg(target_os = "macos")]
    {
        let hidden = cfg().hidden_calendars;
        mac::calendars()
            .into_iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "name": c.name,
                    "color": c.color,
                    "source": c.source,
                    "enabled": !hidden.contains(&c.id),
                })
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Access state plus the calendar list — one call for the Settings panel.
pub fn status() -> Result<Value> {
    Ok(json!({
        "access": access_state().as_str(),
        "account": ACCOUNT,
        "calendars": calendars(),
    }))
}

/// Events between two `YYYY-MM-DD` days (inclusive), shaped like `RangeEvent`
/// so the Calendar view, Today, reminders, and meeting detection can consume
/// them exactly as they consume Google's.
pub fn events_range(start_date: &str, end_date: &str) -> Result<Vec<Value>> {
    if !access_state().can_read() {
        return Ok(Vec::new());
    }
    #[cfg(target_os = "macos")]
    {
        mac::events_range(start_date, end_date, &cfg().hidden_calendars)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (start_date, end_date);
        Ok(Vec::new())
    }
}

/// The day's events in the `CalEvent` shape the daily schedule consumes.
///
/// Derived from `events_range` rather than issuing a second EventKit query: one
/// place knows how to talk to EventKit, so the schedule and the calendar grid
/// can never disagree about what is on the day.
pub fn list_events(event_date: &str) -> Result<Vec<Value>> {
    Ok(events_range(event_date, event_date)?
        .into_iter()
        .filter(|e| !e["declined"].as_bool().unwrap_or(false))
        .map(to_cal_event)
        .collect())
}

/// Minutes from local midnight as "HH:MM". `end_min` can exceed 1440 when an
/// event runs past midnight, so it wraps to the wall-clock time rather than
/// rendering "25:30".
fn hhmm(minutes: i64) -> String {
    let m = minutes.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// `RangeEvent` -> `CalEvent`: the daily schedule wants wall-clock strings where
/// the calendar grid wants minute offsets.
fn to_cal_event(e: Value) -> Value {
    let all_day = e["all_day"].as_bool().unwrap_or(false);
    let at = |key: &str| {
        if all_day {
            Value::Null
        } else {
            e[key].as_i64().map(hhmm).map_or(Value::Null, Value::String)
        }
    };
    json!({
        "id": e["id"],
        "task": e["title"],
        "start": at("start_min"),
        "end": at("end_min"),
        "all_day": all_day,
        "calendar": e["calendar"],
        "calendar_id": e["calendar_id"],
        "color": e["color"],
        "account": e["account"],
        "meet_link": e["meet_link"],
        // Apple events have no web permalink to open.
        "html_link": Value::Null,
    })
}

/// Local midnight bounds for an inclusive `YYYY-MM-DD` range, as unix seconds.
/// Shared with the tests so the window is verified without EventKit.
fn range_bounds(start_date: &str, end_date: &str) -> Result<(f64, f64)> {
    let tz = crate::system_settings::time_zone();
    let start = chrono::NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
        .map_err(|e| anyhow!("bad start date {start_date}: {e}"))?;
    let end = chrono::NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
        .map_err(|e| anyhow!("bad end date {end_date}: {e}"))?;
    if end < start {
        return Err(anyhow!("end date {end_date} precedes start {start_date}"));
    }
    // Inclusive of the whole end day, so an event at 23:30 on the last day is
    // still inside the window.
    let from = tz
        .from_local_datetime(&start.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .ok_or_else(|| anyhow!("no valid local start for {start_date}"))?;
    let to = tz
        .from_local_datetime(&(end + chrono::Days::new(1)).and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .ok_or_else(|| anyhow!("no valid local end for {end_date}"))?;
    Ok((from.timestamp() as f64, to.timestamp() as f64))
}

/// Turn an absolute start/end into the day-plus-minutes form `RangeEvent` uses.
/// Pure, so the all-day and multi-day cases are testable without EventKit.
fn local_span(start_unix: f64, end_unix: f64, all_day: bool) -> Value {
    let tz = crate::system_settings::time_zone();
    let s = tz.timestamp_opt(start_unix as i64, 0).unwrap();
    let e = tz.timestamp_opt(end_unix as i64, 0).unwrap();
    if all_day {
        // EventKit's all-day end is the last day at 00:00 local; Google's
        // end_date is exclusive-free, so report the final covered day.
        let last = if e.date_naive() > s.date_naive() {
            e.date_naive() - chrono::Days::new(1)
        } else {
            s.date_naive()
        };
        return json!({
            "date": s.format("%Y-%m-%d").to_string(),
            "end_date": (last > s.date_naive()).then(|| last.format("%Y-%m-%d").to_string()),
            "start_min": Value::Null,
            "end_min": Value::Null,
            "all_day": true,
        });
    }
    let start_min = (s.hour() * 60 + s.minute()) as i64;
    let days = (e.date_naive() - s.date_naive()).num_days();
    // Zero/negative spans still get a visible, clickable block (matches gcal).
    let end_min = (days * 1440 + (e.hour() * 60 + e.minute()) as i64).max(start_min + 15);
    json!({
        "date": s.format("%Y-%m-%d").to_string(),
        "end_date": Value::Null,
        "start_min": start_min,
        "end_min": end_min,
        "all_day": false,
    })
}

/// Meeting join link from an event's URL, location, or notes — the same places
/// Google's conference data ends up when the invite came from elsewhere.
fn meeting_link(url: Option<&str>, location: Option<&str>, notes: Option<&str>) -> Option<String> {
    const HOSTS: [&str; 6] = [
        "zoom.us",
        "meet.google.com",
        "teams.microsoft.com",
        "teams.live.com",
        "webex.com",
        "whereby.com",
    ];
    let looks_like_call = |candidate: &str| {
        let lower = candidate.to_lowercase();
        HOSTS.iter().any(|host| lower.contains(host))
    };
    if let Some(url) = url.filter(|u| looks_like_call(u)) {
        return Some(url.to_string());
    }
    // Fall back to scanning free text for the first call URL.
    for field in [location, notes].into_iter().flatten() {
        for token in field.split_whitespace() {
            let token =
                token.trim_matches(|c: char| !c.is_ascii_graphic() || "<>(),;\"'".contains(c));
            if token.starts_with("http") && looks_like_call(token) {
                return Some(token.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use block2::RcBlock;
    use objc2_event_kit::{
        EKAuthorizationStatus, EKCalendar, EKEntityType, EKEvent, EKEventStatus, EKEventStore,
        EKEventStoreRequestAccessCompletionHandler, EKParticipantStatus,
    };
    use objc2_foundation::{NSArray, NSDate};

    pub struct CalendarInfo {
        pub id: String,
        pub name: String,
        pub color: String,
        pub source: String,
    }

    /// Full access landed in macOS 14; older systems use the pre-14 grant. The
    /// deployment target is 10.15, so both paths have to exist.
    fn full_access_available() -> bool {
        // `requestFullAccessToEventsWithCompletion:` is the macOS 14 selector.
        objc2::class!(EKEventStore)
            .responds_to(objc2::sel!(requestFullAccessToEventsWithCompletion:))
    }

    pub fn access_state() -> Access {
        let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        match status {
            EKAuthorizationStatus::FullAccess => Access::Granted,
            EKAuthorizationStatus::Denied => Access::Denied,
            EKAuthorizationStatus::Restricted => Access::Restricted,
            EKAuthorizationStatus::WriteOnly => Access::WriteOnly,
            _ => Access::NotDetermined,
        }
    }

    /// Re-read the access state until TCC's decision becomes visible.
    ///
    /// The grant is not always readable by this process the instant the
    /// completion block fires, and reporting a stale `NotDetermined` over a
    /// grant the user just made would send them back to click Allow again.
    async fn settled_access() -> Access {
        const INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
        const ATTEMPTS: u32 = 30; // 3s, far longer than the observed lag
        for _ in 0..ATTEMPTS {
            let state = access_state();
            if state != Access::NotDetermined {
                return state;
            }
            tokio::time::sleep(INTERVAL).await;
        }
        access_state()
    }

    pub async fn request_access(app: &tauri::AppHandle) -> Result<Access> {
        /// How long to leave the OS prompt on screen before giving up on it.
        const WAIT: std::time::Duration = std::time::Duration::from_secs(120);
        let before = access_state();
        if before.can_read() {
            return Ok(before);
        }
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        // TCC presents its prompt through the app's UI, so the request has to
        // originate on the main thread — asked from a detached thread it is
        // silently dropped and the user sees nothing at all. The completion
        // block is !Send, so it is built *inside* this closure rather than moved
        // into it; only the channel sender crosses the boundary.
        let tx = std::sync::Mutex::new(Some(tx));
        app.run_on_main_thread(move || {
            let completion = RcBlock::new(
                move |granted: objc2::runtime::Bool, err: *mut objc2_foundation::NSError| {
                    // EventKit can decline without ever prompting (no usage
                    // description, a restricted profile, an unattributable
                    // launch). The NSError is the only thing that says which,
                    // so never swallow it.
                    let reason = if err.is_null() {
                        String::new()
                    } else {
                        format!(" error: {}", unsafe { &*err }.localizedDescription())
                    };
                    log_line(&format!(
                        "eventkit replied granted={}{reason}",
                        granted.as_bool()
                    ));
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                },
            );
            // Deref through the RcBlock to the block itself. Passing
            // `&completion` hands EventKit a pointer to the smart pointer
            // rather than the block, and it dies in objc_msgSend on garbage.
            let handler: EKEventStoreRequestAccessCompletionHandler =
                &*completion as *const _ as *mut _;
            with_store(|store| unsafe {
                if full_access_available() {
                    store.requestFullAccessToEventsWithCompletion(handler);
                } else {
                    // Deprecated since macOS 14 but still the only grant
                    // available below it, and the deployment target is 10.15.
                    #[allow(deprecated)]
                    store.requestAccessToEntityType_completion(EKEntityType::Event, handler);
                }
            });
            // The block must outlive this closure: EventKit calls it back long
            // after we return, and a dropped block is freed memory. One block
            // per install is a cheaper price than a global kept alive for it.
            // (The store needs no such care — it is the process-wide one.)
            std::mem::forget(completion);
        })?;

        // Never hang the command if TCC never answers.
        let answered = tokio::time::timeout(WAIT, rx).await.is_ok();
        let after = settled_access().await;
        log_line(&format!(
            "access {} -> {} (callback fired: {answered})",
            before.as_str(),
            after.as_str()
        ));
        Ok(after)
    }

    /// EventKit hands back a CGColor; the frontend wants a hex string like the
    /// one Google supplies, so components are converted directly.
    fn hex_color(cal: &EKCalendar) -> String {
        const FALLBACK: &str = "#8e8e93";
        let Some(color) = (unsafe { cal.CGColor() }) else {
            return FALLBACK.to_string();
        };
        let count = objc2_core_graphics::CGColor::number_of_components(Some(&color));
        let comps = objc2_core_graphics::CGColor::components(Some(&color));
        // A greyscale or pattern colour space has fewer than three channels;
        // rather than read past the buffer, fall back to a neutral swatch.
        if comps.is_null() || count < 3 {
            return FALLBACK.to_string();
        }
        let channel = |i: isize| -> u8 {
            let v = unsafe { *comps.offset(i) };
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        format!("#{:02x}{:02x}{:02x}", channel(0), channel(1), channel(2))
    }

    /// The one event store for the whole process.
    ///
    /// Every `EKEventStore` opens its own XPC connection to the calendar
    /// daemon. Creating one per query wedges that daemon after a handful of
    /// rapid calls: it starts returning an empty calendar list and only
    /// restarting the app clears it. That is exactly what happened when a user
    /// clicked through calendar filters — nine stores in eight seconds, then
    /// every read came back empty. Apple's guidance is one long-lived store.
    struct SharedStore(objc2::rc::Retained<EKEventStore>);

    // SAFETY: the store is reachable only through `with_store`, which holds the
    // mutex for the whole borrow, so it is never touched from two threads at
    // once and no reference escapes the lock.
    unsafe impl Send for SharedStore {}

    fn with_store<T>(f: impl FnOnce(&EKEventStore) -> T) -> T {
        static STORE: OnceLock<std::sync::Mutex<SharedStore>> = OnceLock::new();
        let cell = STORE
            .get_or_init(|| std::sync::Mutex::new(SharedStore(unsafe { EKEventStore::new() })));
        // A panic in one query must not poison calendar reads for the rest of
        // the session; the store itself is unaffected by an unwind.
        let guard = cell.lock().unwrap_or_else(|e| e.into_inner());
        // A long-lived store serves the snapshot it last cached, so refresh it
        // before reading — this is also what picks up a newly granted
        // authorization without waiting for a restart.
        unsafe { guard.0.reset() };
        f(&guard.0)
    }

    pub fn calendars() -> Vec<CalendarInfo> {
        if !super::access_state().can_read() {
            return Vec::new();
        }
        let cals: objc2::rc::Retained<NSArray<EKCalendar>> =
            with_store(|store| unsafe { store.calendarsForEntityType(EKEntityType::Event) });
        if cals.is_empty() {
            // Access is granted, so an empty list means EventKit itself stopped
            // answering — worth a line, unlike the healthy case.
            super::log_line("granted but EventKit returned no calendars");
        }
        cals.iter()
            .map(|cal| CalendarInfo {
                id: unsafe { cal.calendarIdentifier() }.to_string(),
                name: unsafe { cal.title() }.to_string(),
                color: hex_color(&cal),
                source: unsafe { cal.source() }
                    .map(|s| unsafe { s.title() }.to_string())
                    .unwrap_or_default(),
            })
            .collect()
    }

    fn participant_status(status: EKParticipantStatus) -> &'static str {
        match status {
            EKParticipantStatus::Accepted => "accepted",
            EKParticipantStatus::Declined => "declined",
            EKParticipantStatus::Tentative => "tentative",
            _ => "needsAction",
        }
    }

    /// EKParticipant exposes an address only as a `mailto:` URL.
    fn participant_email(url: &str) -> String {
        url.strip_prefix("mailto:").unwrap_or(url).to_lowercase()
    }

    pub fn events_range(start_date: &str, end_date: &str, hidden: &[String]) -> Result<Vec<Value>> {
        let (from, to) = super::range_bounds(start_date, end_date)?;
        // One store, one lock, for the whole query: the calendars, the
        // predicate, and the events must all come from the same store or
        // EventKit treats the fetched objects as invalid.
        with_store(|store| {
            let all: objc2::rc::Retained<NSArray<EKCalendar>> =
                unsafe { store.calendarsForEntityType(EKEntityType::Event) };
            let wanted: Vec<objc2::rc::Retained<EKCalendar>> = all
                .iter()
                .filter(|cal| {
                    let id = unsafe { cal.calendarIdentifier() }.to_string();
                    !hidden.contains(&id)
                })
                .collect();
            if wanted.is_empty() {
                return Ok(Vec::new());
            }
            let refs: Vec<&EKCalendar> = wanted.iter().map(|c| c.as_ref()).collect();
            let cal_array = NSArray::from_slice(&refs);
            let start = NSDate::dateWithTimeIntervalSince1970(from);
            let end = NSDate::dateWithTimeIntervalSince1970(to);
            let predicate = unsafe {
                store.predicateForEventsWithStartDate_endDate_calendars(
                    &start,
                    &end,
                    Some(&cal_array),
                )
            };
            let events: objc2::rc::Retained<NSArray<EKEvent>> =
                unsafe { store.eventsMatchingPredicate(&predicate) };

            let mut out = Vec::new();
            for ev in events.iter() {
                out.push(to_range_event(&ev));
            }
            Ok(out)
        })
    }

    fn to_range_event(ev: &EKEvent) -> Value {
        let all_day = unsafe { ev.isAllDay() };
        let start = unsafe { ev.startDate() }.timeIntervalSince1970();
        let end = unsafe { ev.endDate() }.timeIntervalSince1970();
        let span = super::local_span(start, end, all_day);

        let calendar = unsafe { ev.calendar() };
        let (cal_name, cal_id, color) = calendar
            .as_ref()
            .map(|c| {
                (
                    unsafe { c.title() }.to_string(),
                    unsafe { c.calendarIdentifier() }.to_string(),
                    hex_color(c),
                )
            })
            .unwrap_or_default();

        let location = unsafe { ev.location() }.map(|s| s.to_string());
        let notes = unsafe { ev.notes() }.map(|s| s.to_string());
        let url = unsafe { ev.URL() }.map(|u| u.absoluteString().map(|s| s.to_string()));
        let url = url.flatten();
        let link = super::meeting_link(url.as_deref(), location.as_deref(), notes.as_deref());

        let organizer = unsafe { ev.organizer() };
        let organizer_email = organizer
            .as_ref()
            .map(|p| participant_email(&unsafe { p.URL() }.absoluteString().unwrap().to_string()));
        let organizer_name = organizer
            .as_ref()
            .and_then(|p| unsafe { p.name() }.map(|n| n.to_string()))
            .or_else(|| organizer_email.clone());

        let mut attendees = Vec::new();
        let mut attendee_emails = Vec::new();
        let mut declined_self = false;
        if let Some(list) = unsafe { ev.attendees() } {
            for p in list.iter() {
                let email = unsafe { p.URL() }
                    .absoluteString()
                    .map(|s| participant_email(&s.to_string()))
                    .unwrap_or_default();
                let name = unsafe { p.name() }
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| email.clone());
                let status = participant_status(unsafe { p.participantStatus() });
                let is_self = unsafe { p.isCurrentUser() };
                if is_self && status == "declined" {
                    declined_self = true;
                }
                if !email.is_empty() {
                    attendee_emails.push(email.clone());
                }
                // Matches gcal's cap so the UI never renders an unbounded list.
                if attendees.len() < 12 {
                    attendees.push(json!({
                        "name": name,
                        "email": email,
                        "status": status,
                        "self": is_self,
                    }));
                }
            }
        }

        let cancelled = unsafe { ev.status() } == EKEventStatus::Canceled;
        let mut out = json!({
            "id": unsafe { ev.eventIdentifier() }.map(|s| s.to_string()).unwrap_or_default(),
            "title": unsafe { ev.title() }.to_string(),
            "calendar": cal_name,
            "calendar_id": cal_id,
            "color": color,
            "account": super::ACCOUNT,
            "location": location,
            "description": notes,
            "declined": declined_self || cancelled,
            // Apple events never carry Google conference data, so an edit could
            // not preserve it — the Calendar view uses this to decide what an
            // edit form may offer.
            "google_meet": false,
            "meet_link": link,
            "html_link": Value::Null,
            "organizer": organizer_name,
            "organizer_email": organizer_email,
            "creator_email": Value::Null,
            "attendees": attendees,
            "attendee_count": attendee_emails.len(),
            "attendee_emails": attendee_emails,
            // Read-only: nothing in the app may try to edit these.
            "read_only": true,
        });
        if let Value::Object(span) = span {
            for (k, v) in span {
                out[k] = v;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timed_event_becomes_a_day_plus_minute_offsets() {
        // 2026-06-02 09:30 -> 10:30 UTC (tests run with the default zone).
        let start = chrono::Utc
            .with_ymd_and_hms(2026, 6, 2, 9, 30, 0)
            .unwrap()
            .timestamp() as f64;
        let span = local_span(start, start + 3600.0, false);
        assert_eq!(span["all_day"], json!(false));
        assert!(span["start_min"].is_i64());
        assert_eq!(
            span["end_min"].as_i64().unwrap() - span["start_min"].as_i64().unwrap(),
            60
        );
    }

    #[test]
    fn a_zero_length_event_still_gets_a_clickable_block() {
        // Matches gcal: a degenerate span must not collapse to an invisible row.
        let start = chrono::Utc
            .with_ymd_and_hms(2026, 6, 2, 9, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let span = local_span(start, start, false);
        assert_eq!(
            span["end_min"].as_i64().unwrap() - span["start_min"].as_i64().unwrap(),
            15
        );
    }

    #[test]
    fn a_single_all_day_event_reports_no_end_date() {
        // EventKit ends an all-day event at 00:00 on the following day; that
        // must not surface as a two-day event.
        let start = chrono::Utc
            .with_ymd_and_hms(2026, 6, 2, 0, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let span = local_span(start, start + 86_400.0, true);
        assert_eq!(span["all_day"], json!(true));
        assert_eq!(span["end_date"], Value::Null);
        assert_eq!(span["start_min"], Value::Null);
    }

    #[test]
    fn the_range_covers_the_whole_final_day() {
        let (from, to) = range_bounds("2026-06-02", "2026-06-04").unwrap();
        // Three inclusive days = 72h, so 23:30 on the last day is still inside.
        assert_eq!(to - from, 3.0 * 86_400.0);
    }

    #[test]
    fn a_backwards_range_is_rejected_rather_than_silently_empty() {
        assert!(range_bounds("2026-06-04", "2026-06-02").is_err());
        assert!(range_bounds("not-a-date", "2026-06-02").is_err());
    }

    #[test]
    fn wall_clock_wraps_past_midnight_instead_of_reading_25_30() {
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(9 * 60 + 5), "09:05");
        // 01:30 the next morning, not "25:30".
        assert_eq!(hhmm(25 * 60 + 30), "01:30");
    }

    #[test]
    fn an_all_day_event_carries_no_clock_times() {
        let cal = to_cal_event(json!({
            "id": "x", "title": "Offsite", "all_day": true,
            "start_min": Value::Null, "end_min": Value::Null,
            "calendar": "Work", "calendar_id": "c", "color": "#fff",
            "account": ACCOUNT, "meet_link": Value::Null,
        }));
        assert_eq!(cal["all_day"], json!(true));
        assert_eq!(cal["start"], Value::Null);
        assert_eq!(cal["task"], json!("Offsite"));
    }

    #[test]
    fn a_timed_event_becomes_wall_clock_strings() {
        let cal = to_cal_event(json!({
            "id": "x", "title": "Standup", "all_day": false,
            "start_min": 545, "end_min": 560,
            "calendar": "Work", "calendar_id": "c", "color": "#fff",
            "account": ACCOUNT, "meet_link": Value::Null,
        }));
        assert_eq!(cal["start"], json!("09:05"));
        assert_eq!(cal["end"], json!("09:20"));
    }

    #[test]
    fn call_links_are_found_in_url_location_or_notes() {
        assert_eq!(
            meeting_link(Some("https://zoom.us/j/123"), None, None).as_deref(),
            Some("https://zoom.us/j/123")
        );
        assert_eq!(
            meeting_link(
                None,
                Some("Room 4 https://meet.google.com/abc-defg-hij"),
                None
            )
            .as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        // A non-call URL must not be offered as a join link.
        assert_eq!(
            meeting_link(Some("https://example.com/agenda"), None, None),
            None
        );
        assert_eq!(meeting_link(None, None, None), None);
    }
}
