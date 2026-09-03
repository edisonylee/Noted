//! Audible reminders for timed calendar events and local schedule blocks.
//!
//! The worker lives in the Rust process rather than a React view, so reminders
//! continue while the main window is hidden. Google events are refreshed every
//! five minutes; local plans are read on every pass so edits take effect quickly.

use anyhow::{anyhow, Result};
use chrono::{NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

const CONFIG_FILE: &str = "reminders.json";
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
const CALENDAR_REFRESH: Duration = Duration::from_secs(5 * 60);

fn default_lead_minutes() -> i64 {
    10
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReminderSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_lead_minutes")]
    pub lead_minutes: i64,
}

impl Default for ReminderSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            lead_minutes: default_lead_minutes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReminderItem {
    id: String,
    title: String,
    date: String,
    start_min: i64,
    meeting: bool,
}

static SETTINGS: OnceLock<RwLock<ReminderSettings>> = OnceLock::new();

fn settings_cell() -> &'static RwLock<ReminderSettings> {
    SETTINGS.get_or_init(|| RwLock::new(ReminderSettings::default()))
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

fn validate(settings: &ReminderSettings) -> Result<()> {
    if !(1..=120).contains(&settings.lead_minutes) {
        return Err(anyhow!(
            "reminder lead time must be between 1 and 120 minutes"
        ));
    }
    Ok(())
}

pub fn init(dir: &Path) {
    let loaded = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<ReminderSettings>(&raw).ok())
        .filter(|settings| validate(settings).is_ok())
        .unwrap_or_default();
    *settings_cell().write().unwrap() = loaded;
}

pub fn get() -> ReminderSettings {
    settings_cell().read().unwrap().clone()
}

pub fn update(dir: &Path, settings: ReminderSettings) -> Result<ReminderSettings> {
    validate(&settings)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&settings)?)?;
    std::fs::rename(tmp, config_path(dir))?;
    *settings_cell().write().unwrap() = settings.clone();
    Ok(settings)
}

fn normalized_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn occurrence_key(item: &ReminderItem) -> String {
    format!(
        "{}|{}|{}",
        item.date,
        item.start_min,
        normalized_title(&item.title)
    )
}

fn calendar_items(events: Vec<Value>) -> Vec<ReminderItem> {
    events
        .into_iter()
        .filter(|event| !event["declined"].as_bool().unwrap_or(false))
        .filter(|event| !event["all_day"].as_bool().unwrap_or(false))
        // The dedicated noted calendar mirrors local plans. Reading it here
        // would produce two alerts for the same schedule block.
        .filter(|event| event["calendar"].as_str().unwrap_or("").to_lowercase() != "noted")
        .filter_map(|event| {
            let title = event["title"].as_str()?.trim().to_string();
            let date = event["date"].as_str()?.to_string();
            let start_min = event["start_min"].as_i64()?;
            if title.is_empty() {
                return None;
            }
            let meeting = event["meet_link"].is_string()
                || event["attendee_count"].as_i64().unwrap_or(0) >= 2;
            Some(ReminderItem {
                id: format!("calendar:{}", event["id"].as_str().unwrap_or("event")),
                title,
                date,
                start_min,
                meeting,
            })
        })
        .collect()
}

fn local_plan_items(app: &tauri::AppHandle, dates: &[String]) -> Vec<ReminderItem> {
    let state = app.state::<crate::db::Db>();
    let conn = state.0.lock().unwrap();
    let mut items = Vec::new();
    for date in dates {
        let Ok(blocks) = crate::db::schedule_blocks_for(&conn, date) else {
            continue;
        };
        for (index, block) in blocks.into_iter().enumerate() {
            let Some(title) = block.get("task").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            let Some(start_min) = block
                .get("start")
                .and_then(Value::as_str)
                .and_then(parse_hhmm)
            else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            items.push(ReminderItem {
                id: format!("plan:{date}:{index}"),
                title: title.to_string(),
                date: date.clone(),
                start_min,
                meeting: false,
            });
        }
    }
    items
}

fn parse_hhmm(value: &str) -> Option<i64> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour = hour.parse::<i64>().ok()?;
    let minute = minute.parse::<i64>().ok()?;
    ((0..24).contains(&hour) && (0..60).contains(&minute)).then_some(hour * 60 + minute)
}

fn seconds_until(item: &ReminderItem) -> Option<i64> {
    let date = NaiveDate::parse_from_str(&item.date, "%Y-%m-%d").ok()?;
    let hour = (item.start_min / 60) as u32;
    let minute = (item.start_min % 60) as u32;
    let local = date.and_hms_opt(hour, minute, 0)?;
    let target = crate::system_settings::time_zone()
        .from_local_datetime(&local)
        .earliest()?;
    Some(
        target
            .signed_duration_since(crate::now_local())
            .num_seconds(),
    )
}

fn clock_label(start_min: i64) -> String {
    let hour = start_min / 60;
    let minute = start_min % 60;
    let suffix = if hour >= 12 { "PM" } else { "AM" };
    let hour_12 = match hour % 12 {
        0 => 12,
        value => value,
    };
    format!("{hour_12}:{minute:02} {suffix}")
}

fn notify(app: &tauri::AppHandle, item: &ReminderItem, seconds: i64) -> Result<()> {
    // Round up so an item 9m01s away still reads "in 10 minutes" rather than
    // appearing late. The default macOS sound respects Focus and the user's
    // per-app notification sound preference.
    let minutes = ((seconds + 59) / 60).max(1);
    let kind = if item.meeting { "Meeting" } else { "Plan" };
    app.notification()
        .builder()
        .title(format!(
            "{kind} in {minutes} minute{}",
            if minutes == 1 { "" } else { "s" }
        ))
        .body(format!("{} · {}", item.title, clock_label(item.start_min)))
        .sound("Ping")
        .show()
        .map_err(|error| anyhow!(error.to_string()))
}

fn merge_items(calendar: &[ReminderItem], local: Vec<ReminderItem>) -> Vec<ReminderItem> {
    let mut by_occurrence: HashMap<String, ReminderItem> = calendar
        .iter()
        .cloned()
        .map(|item| (occurrence_key(&item), item))
        .collect();
    for item in local {
        by_occurrence.entry(occurrence_key(&item)).or_insert(item);
    }
    by_occurrence.into_values().collect()
}

async fn fetch_calendar(app: &tauri::AppHandle, start: &str, end: &str) -> Vec<ReminderItem> {
    let Ok(dir) = app.path().app_data_dir() else {
        return Vec::new();
    };
    crate::calendar::events_range(&dir, start, end)
        .await
        .map(calendar_items)
        .unwrap_or_default()
}

pub fn spawn(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("noted-reminders".into())
        .spawn(move || run(app))
        .expect("failed to start reminder worker");
}

fn run(app: tauri::AppHandle) {
    let mut calendar: Vec<ReminderItem> = Vec::new();
    let mut calendar_fetched_at: Option<Instant> = None;
    let mut notified: HashSet<String> = HashSet::new();
    let mut notified_day = crate::today_local();

    loop {
        let settings = get();
        if settings.enabled {
            let today = crate::today_local();
            let tomorrow = (crate::now_local().date_naive() + chrono::Days::new(1)).to_string();
            if today != notified_day {
                notified.clear();
                notified_day = today.clone();
            }

            if calendar_fetched_at
                .map(|at| at.elapsed() >= CALENDAR_REFRESH)
                .unwrap_or(true)
            {
                calendar = tauri::async_runtime::block_on(fetch_calendar(&app, &today, &tomorrow));
                calendar_fetched_at = Some(Instant::now());
            }

            let local = local_plan_items(&app, &[today, tomorrow]);
            for item in merge_items(&calendar, local) {
                let key = format!("{}|{}", item.id, occurrence_key(&item));
                if notified.contains(&key) {
                    continue;
                }
                let Some(seconds) = seconds_until(&item) else {
                    continue;
                };
                if seconds > 0 && seconds <= settings.lead_minutes * 60 {
                    // Claim before showing so a transient OS delivery error
                    // cannot produce a sound every 30 seconds.
                    notified.insert(key);
                    if let Err(error) = notify(&app, &item, seconds) {
                        eprintln!("[noted] reminder notification failed: {error}");
                    }
                }
            }
        } else {
            // Refresh immediately after the user turns reminders back on.
            calendar_fetched_at = None;
        }
        std::thread::sleep(CHECK_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn calendar_events_become_meetings_or_plans() {
        let items = calendar_items(vec![
            json!({
                "id": "meet",
                "title": "Design review",
                "date": "2026-08-14",
                "start_min": 600,
                "all_day": false,
                "declined": false,
                "calendar": "Work",
                "meet_link": "https://meet.google.com/example",
                "attendee_count": 2
            }),
            json!({
                "id": "focus",
                "title": "Write proposal",
                "date": "2026-08-14",
                "start_min": 660,
                "all_day": false,
                "declined": false,
                "calendar": "Personal",
                "meet_link": null,
                "attendee_count": 0
            }),
        ]);
        assert_eq!(items.len(), 2);
        assert!(items[0].meeting);
        assert!(!items[1].meeting);
    }

    #[test]
    fn mirrored_plan_is_not_notified_twice() {
        let calendar = vec![ReminderItem {
            id: "calendar:one".into(),
            title: "Design review".into(),
            date: "2026-08-14".into(),
            start_min: 600,
            meeting: true,
        }];
        let local = vec![ReminderItem {
            id: "plan:2026-08-14:0".into(),
            title: "  DESIGN   review ".into(),
            date: "2026-08-14".into(),
            start_min: 600,
            meeting: false,
        }];
        let merged = merge_items(&calendar, local);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].meeting);
    }

    #[test]
    fn validates_lead_time_and_clock_values() {
        assert_eq!(parse_hhmm("09:05"), Some(545));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("-1:00"), None);
        assert!(validate(&ReminderSettings {
            enabled: true,
            lead_minutes: 0
        })
        .is_err());
        assert!(validate(&ReminderSettings {
            enabled: true,
            lead_minutes: 30
        })
        .is_ok());
    }
}
