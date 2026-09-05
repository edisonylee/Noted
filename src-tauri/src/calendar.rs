//! Provider-neutral calendar reads.
//!
//! `gcal` and `applecal` each know one backend; this is the only place that
//! knows there is more than one. Everything downstream — the Calendar view, the
//! daily schedule, reminders, and meeting detection — reads through here, so
//! adding a provider does not touch any consumer.
//!
//! Writes are deliberately absent: Apple Calendar is read-only for now, so
//! event creation and the schedule push stay on `gcal`'s own commands where the
//! Google-shaped contract still holds.

use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Combine one provider's events with the other's.
///
/// A single provider failing must not blank the view: Google being offline
/// should still leave local Apple events on screen, and an Apple-only user must
/// never see "not connected to Google Calendar" — which is what `gcal` returns
/// when no account is configured. So Google's error is propagated only when
/// Apple cannot serve the request either, preserving the previous behaviour for
/// anyone who has not connected Apple at all.
///
/// Availability is asked of `applecal`, not inferred from an empty result: a
/// genuinely empty day is not the same as a provider that cannot answer.
fn merge(google: Result<Vec<Value>>, apple: Vec<Value>, what: &str) -> Result<Vec<Value>> {
    match google {
        Ok(mut events) => {
            events.extend(apple);
            Ok(events)
        }
        Err(e) if crate::applecal::access_state().can_read() => {
            eprintln!("[noted] google {what} failed, showing Apple events only: {e}");
            Ok(apple)
        }
        Err(e) => Err(e),
    }
}

/// Apple events for a call that must not fail the whole read.
fn apple_or_empty(events: Result<Vec<Value>>, what: &str) -> Vec<Value> {
    events.unwrap_or_else(|e| {
        eprintln!("[noted] apple {what} failed: {e}");
        Vec::new()
    })
}

/// Every visible calendar's events between two `YYYY-MM-DD` days, inclusive.
/// Positioned by minute offsets — the Calendar grid's shape.
pub async fn events_range(dir: &Path, start_date: &str, end_date: &str) -> Result<Vec<Value>> {
    let apple = apple_or_empty(
        crate::applecal::events_range(start_date, end_date),
        "events_range",
    );
    merge(
        crate::gcal::events_range(dir, start_date, end_date).await,
        apple,
        "events_range",
    )
}

/// One day's events in wall-clock form — the daily schedule's shape.
pub async fn list_events(dir: &Path, event_date: &str) -> Result<Vec<Value>> {
    let apple = apple_or_empty(crate::applecal::list_events(event_date), "list_events");
    merge(
        crate::gcal::list_events(dir, event_date).await,
        apple,
        "list_events",
    )
}
