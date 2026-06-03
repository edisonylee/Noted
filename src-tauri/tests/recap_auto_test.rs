// Auto-recap building blocks: calendar boundaries (pure) + recap existence/idempotency (db).
use chrono::NaiveDate;
use tauri_app_lib::db;
use tauri_app_lib::{last_completed_week, recent_completed_days};

#[test]
fn calendar_boundaries() {
    // 2026-06-01 is a Monday; 2026-06-03 a Wednesday; 2026-06-07 a Sunday.
    let wed = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap();
    assert_eq!(
        last_completed_week(wed),
        ("2026-05-25".to_string(), "2026-05-31".to_string())
    );
    assert_eq!(
        recent_completed_days(wed, 3),
        vec!["2026-06-02".to_string(), "2026-06-01".to_string(), "2026-05-31".to_string()]
    );

    // On Monday, last completed week is the same prior Mon–Sun.
    let mon = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    assert_eq!(last_completed_week(mon), ("2026-05-25".to_string(), "2026-05-31".to_string()));

    // On Sunday, this week (Mon 6/1) isn't done yet → last week still 5/25–5/31.
    let sun = NaiveDate::from_ymd_opt(2026, 6, 7).unwrap();
    assert_eq!(last_completed_week(sun), ("2026-05-25".to_string(), "2026-05-31".to_string()));
}

#[test]
fn recap_exists_and_idempotent() {
    let tmp = std::env::temp_dir().join(format!("noted_recapauto_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let conn = db::init(&tmp).unwrap();

    assert!(!db::recap_exists(&conn, "day", "2026-06-02", "2026-06-02").unwrap());
    db::upsert_recap(&conn, "day", "2026-06-02", "2026-06-02", "you trained", 1, "2026-06-03T00:00:00Z").unwrap();
    assert!(db::recap_exists(&conn, "day", "2026-06-02", "2026-06-02").unwrap());
    // upsert again replaces, so still exactly one
    db::upsert_recap(&conn, "day", "2026-06-02", "2026-06-02", "updated", 1, "2026-06-03T01:00:00Z").unwrap();
    let recaps = db::list_recaps(&conn, 10).unwrap();
    assert_eq!(recaps.iter().filter(|r| r.period == "day").count(), 1);

    let _ = std::fs::remove_file(&tmp);
}
