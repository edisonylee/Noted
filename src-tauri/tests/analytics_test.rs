// M4: trend discovery over the flexible per-category schema (pure, no model).
use serde_json::json;
use tauri_app_lib::analytics::build_trends;

#[test]
fn discovers_gym_structure() {
    let entries = vec![
        (
            "2026-06-09".to_string(),
            json!({"exercises":[
                {"name":"bench","weight":185,"sets":3,"reps":5},
                {"name":"incline db press","weight":70,"sets":3,"reps":10}
            ]}),
        ),
        (
            "2026-06-11".to_string(),
            json!({"exercises":[
                {"name":"squat","weight":245,"sets":3,"reps":5,"rpe":9},
                {"name":"bench","weight":190,"sets":3,"reps":5}
            ]}),
        ),
    ];
    let t = build_trends(&entries);
    assert_eq!(t.items_field.as_deref(), Some("exercises"));
    assert_eq!(t.label_field.as_deref(), Some("name"));
    // numeric metrics discovered (union across items), sorted
    assert!(t.metrics.contains(&"weight".to_string()));
    assert!(t.metrics.contains(&"sets".to_string()));
    assert!(t.metrics.contains(&"rpe".to_string()));
    // labels in first-appearance order
    assert_eq!(t.labels, vec!["bench", "incline db press", "squat"]);
    // bench appears on both dates -> two rows for it
    let bench_rows: Vec<_> = t.rows.iter().filter(|r| r.label == "bench").collect();
    assert_eq!(bench_rows.len(), 2);
    assert_eq!(bench_rows[1].values.get("weight").unwrap(), &json!(190));
    assert_eq!(t.count_by_date, vec![("2026-06-09".into(), 1), ("2026-06-11".into(), 1)]);
}

#[test]
fn handles_flat_category() {
    // meals: no array-of-objects; the data object itself is the item
    let entries = vec![
        ("2026-06-09".to_string(), json!({"meal":"burrito bowl","calories":750})),
        ("2026-06-10".to_string(), json!({"meal":"salad","calories":420})),
    ];
    let t = build_trends(&entries);
    assert_eq!(t.items_field, None);
    assert_eq!(t.label_field.as_deref(), Some("meal"));
    assert_eq!(t.metrics, vec!["calories".to_string()]);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].values.get("calories").unwrap(), &json!(750));
}
