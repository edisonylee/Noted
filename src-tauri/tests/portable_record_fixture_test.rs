use serde_json::{json, Value};
use tauri_app_lib::portable::{
    canonical_json, canonical_sha256, deterministic_backfill_uuid_v7, ContextRecordV1,
};

fn fixture_uuid(timestamp_ms: u64, key: &str) -> String {
    deterministic_backfill_uuid_v7(timestamp_ms, "portable-record-golden", key)
}

#[test]
fn portable_content_has_stable_canonical_bytes_and_hash() {
    let content = json!({
        "title": "Résumé",
        "body": "Line 1\r\n雪",
        "nested": {"z": [true, null, 1.25], "a": 1}
    });

    assert_eq!(
        canonical_json(&content),
        "{\"body\":\"Line 1\\r\\n雪\",\"nested\":{\"a\":1,\"z\":[true,null,1.25]},\"title\":\"Résumé\"}"
    );
    assert_eq!(
        canonical_sha256(&content),
        "dfdcf4a9cc8870a85f9743ea5599e8b72387ff5359b7d6711149d6ec5f7b26da"
    );
    assert_ne!(
        canonical_sha256(&content),
        canonical_sha256(&json!({
            "title": "Résumé",
            "body": "Line 1\n雪",
            "nested": {"a": 1, "z": [true, null, 1.25]}
        }))
    );
}

#[test]
fn accepted_record_round_trip_retains_unknown_extension() {
    let content = json!({"body":"Portable", "title":"Fixture"});
    let raw = json!({
        "contract_version": "noted.context-record.v1",
        "library_id": fixture_uuid(1_700_000_000_000, "library"),
        "record_id": fixture_uuid(1_700_000_000_001, "record"),
        "kind": "note",
        "record_schema_version": 1,
        "revision": 1,
        "version_id": fixture_uuid(1_700_000_000_002, "version"),
        "created_at": "2026-08-16T00:00:00Z",
        "updated_at": "2026-08-16T00:01:00Z",
        "event_time": {
            "occurred_at": "2026-08-15T18:00:00Z",
            "ended_at": "2026-08-15T19:00:00Z",
            "timezone": "America/Los_Angeles"
        },
        "scope": {
            "scope_id": fixture_uuid(1_700_000_000_003, "scope"),
            "class": "personal"
        },
        "sensitivity": "standard",
        "authority": {"kind": "noted", "origin": "capture"},
        "content": content,
        "content_hash": canonical_sha256(&content),
        "provenance": {"source":"typed"},
        "lifecycle": {"state":"active"},
        "example.vendor/opaque": {"must":"survive"}
    });

    let record: ContextRecordV1 = serde_json::from_value(raw).expect("decode fixture");
    record.validate().expect("validate fixture");
    let encoded = serde_json::to_value(record).expect("encode fixture");
    assert_eq!(encoded["example.vendor/opaque"], json!({"must":"survive"}));
}

#[test]
fn invalid_contract_timestamp_and_record_version_are_rejected() {
    let content = json!({"body":"Portable"});
    let base = json!({
        "contract_version": "noted.context-record.v1",
        "library_id": fixture_uuid(1_700_000_000_000, "library"),
        "record_id": fixture_uuid(1_700_000_000_001, "record"),
        "kind": "note",
        "record_schema_version": 1,
        "revision": 1,
        "version_id": fixture_uuid(1_700_000_000_002, "version"),
        "created_at": "2026-08-16T00:00:00Z",
        "updated_at": "2026-08-16T00:01:00Z",
        "scope": {
            "scope_id": fixture_uuid(1_700_000_000_003, "scope"),
            "class": "personal"
        },
        "sensitivity": "standard",
        "authority": {"kind": "noted"},
        "content": content,
        "content_hash": canonical_sha256(&content),
        "provenance": {},
        "lifecycle": {"state":"active"}
    });

    for (field, invalid) in [
        (
            "contract_version",
            Value::String("noted.context-record.v2".into()),
        ),
        (
            "created_at",
            Value::String("2026-08-16T00:00:00-07:00".into()),
        ),
        ("record_schema_version", Value::from(0)),
        ("revision", Value::from(0)),
    ] {
        let mut candidate = base.clone();
        candidate[field] = invalid;
        let record: ContextRecordV1 = serde_json::from_value(candidate).expect("decode candidate");
        assert!(record.validate().is_err(), "{field} should be rejected");
    }
}
