//! Provider- and database-neutral identities and canonical record envelopes.
//!
//! This module is compiled into both the macOS and iOS targets. SQLite row IDs,
//! local paths, provider identifiers, and transport state do not belong here.

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONTEXT_RECORD_VERSION: &str = "noted.context-record.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    Noted,
    External,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordAuthority {
    pub kind: AuthorityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Trash,
    Tombstone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordLifecycle {
    pub state: LifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstoned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeClass {
    Work,
    Personal,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordScope {
    /// Stable UUID for this logical scope. Display names remain mutable content.
    pub scope_id: String,
    pub class: ScopeClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordEventTime {
    /// RFC 3339 instant. Civil-time records must also include an IANA timezone.
    pub occurred_at: String,
    pub timezone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedHead {
    pub revision: u64,
    pub version_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalBranchState {
    Pending,
    Superseded,
    Conflict,
}

/// Working state is intentionally separate from `ContextRecordV1`: a local
/// offline edit is not an accepted positive revision until the authority has
/// committed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBranch {
    pub branch_id: String,
    pub base_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_version_id: Option<String>,
    pub local_revision: u64,
    pub working_version_id: String,
    pub content_hash: String,
    pub state: LocalBranchState,
}

impl LocalBranch {
    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [
            ("branch_id", Some(self.branch_id.as_str())),
            ("base_version_id", self.base_version_id.as_deref()),
            ("working_version_id", Some(self.working_version_id.as_str())),
        ] {
            if value.is_some_and(|value| !is_uuid(value)) {
                return Err(format!("{label} must be a canonical UUID"));
            }
        }
        if self.base_revision == 0 && self.base_version_id.is_some() {
            return Err("an unaccepted branch cannot have a base version".to_string());
        }
        if self.base_revision > 0 && self.base_version_id.is_none() {
            return Err("an accepted base revision requires a base version".to_string());
        }
        if self.local_revision == 0 || self.content_hash.len() != 64 {
            return Err("a local branch requires a revision and SHA-256 content hash".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRecordV1 {
    pub contract_version: String,
    pub library_id: String,
    pub record_id: String,
    pub kind: String,
    pub record_schema_version: u32,
    pub revision: u64,
    pub version_id: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_time: Option<RecordEventTime>,
    pub scope: RecordScope,
    pub sensitivity: String,
    pub authority: RecordAuthority,
    pub content: Value,
    pub content_hash: String,
    pub provenance: Value,
    pub lifecycle: RecordLifecycle,
    /// Unknown top-level fields are retained on a read/write round trip. A
    /// writer still needs the advertised per-kind capability before changing
    /// a record whose fields it does not understand.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ContextRecordV1 {
    pub fn new(
        library_id: String,
        record_id: String,
        kind: String,
        record_schema_version: u32,
        revision: u64,
        version_id: String,
        created_at: String,
        updated_at: String,
        event_time: Option<RecordEventTime>,
        scope: RecordScope,
        sensitivity: String,
        authority: RecordAuthority,
        content: Value,
        provenance: Value,
        lifecycle: RecordLifecycle,
    ) -> Result<Self, String> {
        let mut record = Self {
            contract_version: CONTEXT_RECORD_VERSION.to_string(),
            library_id,
            record_id,
            kind,
            record_schema_version,
            revision,
            version_id,
            created_at,
            updated_at,
            event_time,
            scope,
            sensitivity,
            authority,
            content_hash: canonical_sha256(&content),
            content,
            provenance,
            lifecycle,
            extensions: BTreeMap::new(),
        };
        record.validate()?;
        // Recompute after validation so callers can never supply or retain a
        // stale hash through construction.
        record.content_hash = canonical_sha256(&record.content);
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != CONTEXT_RECORD_VERSION {
            return Err("unsupported context record version".to_string());
        }
        for (label, value) in [
            ("library_id", self.library_id.as_str()),
            ("version_id", self.version_id.as_str()),
            ("scope_id", self.scope.scope_id.as_str()),
        ] {
            if !is_uuid(value) {
                return Err(format!("{label} must be a canonical UUID"));
            }
        }
        if !is_uuid_v7(&self.record_id) {
            return Err("record_id must be a canonical UUIDv7".to_string());
        }
        if self.kind.trim().is_empty() {
            return Err("record kind cannot be empty".to_string());
        }
        if self.record_schema_version == 0 {
            return Err("record schema version must be positive".to_string());
        }
        if self.revision == 0 {
            return Err("record revision must be positive".to_string());
        }
        let created_at = parse_utc_rfc3339(&self.created_at)
            .ok_or_else(|| "created_at must be canonical UTC RFC 3339".to_string())?;
        let updated_at = parse_utc_rfc3339(&self.updated_at)
            .ok_or_else(|| "updated_at must be canonical UTC RFC 3339".to_string())?;
        if updated_at < created_at {
            return Err("updated_at cannot precede created_at".to_string());
        }
        if let Some(event_time) = &self.event_time {
            let occurred_at = parse_utc_rfc3339(&event_time.occurred_at);
            let ended_at = event_time.ended_at.as_deref().map(parse_utc_rfc3339);
            let timezone_is_valid = event_time.timezone.parse::<chrono_tz::Tz>().is_ok();
            if occurred_at.is_none()
                || ended_at.as_ref().is_some_and(Option::is_none)
                || !timezone_is_valid
            {
                return Err(
                    "event time requires UTC RFC 3339 instant(s) and an IANA timezone".to_string(),
                );
            }
            if let (Some(start), Some(Some(end))) = (occurred_at, ended_at) {
                if end < start {
                    return Err("event end cannot precede event start".to_string());
                }
            }
        }
        if !matches!(
            self.sensitivity.as_str(),
            "standard" | "sensitive" | "restricted"
        ) {
            return Err(
                "record sensitivity must be standard, sensitive, or restricted".to_string(),
            );
        }
        if matches!(
            self.authority.kind,
            AuthorityKind::External | AuthorityKind::Derived
        ) && self
            .authority
            .origin
            .as_deref()
            .is_none_or(|origin| origin.trim().is_empty())
        {
            return Err("external and derived records require an authority origin".to_string());
        }
        if self
            .extensions
            .keys()
            .any(|key| !key.contains('/') || key.starts_with('/') || key.ends_with('/'))
        {
            return Err("extension fields must use a non-empty namespaced key".to_string());
        }
        if self.content_hash != canonical_sha256(&self.content) {
            return Err("record content hash does not match canonical content".to_string());
        }
        let trashed_at = self
            .lifecycle
            .trashed_at
            .as_deref()
            .map(parse_utc_rfc3339)
            .transpose_option("trashed_at must be canonical UTC RFC 3339")?;
        let tombstoned_at = self
            .lifecycle
            .tombstoned_at
            .as_deref()
            .map(parse_utc_rfc3339)
            .transpose_option("tombstoned_at must be canonical UTC RFC 3339")?;
        match (
            &self.lifecycle.state,
            trashed_at.is_some(),
            tombstoned_at.is_some(),
        ) {
            (LifecycleState::Active, false, false)
            | (LifecycleState::Trash, true, false)
            | (LifecycleState::Tombstone, true, true) => {}
            (LifecycleState::Active, _, _) => {
                return Err("an active record cannot have lifecycle timestamps".to_string())
            }
            (LifecycleState::Trash, false, _) => {
                return Err("a trashed record requires trashed_at".to_string())
            }
            (LifecycleState::Trash, _, true) => {
                return Err("a trashed record cannot have tombstoned_at".to_string())
            }
            (LifecycleState::Tombstone, false, _) => {
                return Err("a tombstone requires prior trashed_at".to_string())
            }
            (LifecycleState::Tombstone, _, false) => {
                return Err("a tombstone requires tombstoned_at".to_string())
            }
        }
        if trashed_at.is_some_and(|value| value < created_at) {
            return Err("trashed_at cannot precede created_at".to_string());
        }
        if let (Some(trashed), Some(tombstoned)) = (trashed_at, tombstoned_at) {
            if tombstoned < trashed {
                return Err("tombstoned_at cannot precede trashed_at".to_string());
            }
        }
        Ok(())
    }
}

/// Generate a UUIDv7 identifier using the current Unix-millisecond timestamp
/// and operating-system randomness. The timestamp disclosure is intentional and
/// documented by the portable-record contract.
pub fn new_uuid_v7() -> String {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let mut random = [0u8; 10];
    OsRng.fill_bytes(&mut random);
    uuid_v7_from_parts(timestamp_ms, random)
}

/// Produce the same UUIDv7 every time a legacy row is backfilled. The legacy
/// row key is never exposed directly, while its creation timestamp preserves
/// UUIDv7 ordering. This helper is for one-time migration only, not new writes.
pub fn deterministic_backfill_uuid_v7(
    timestamp_ms: u64,
    namespace: &str,
    stable_legacy_key: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"noted.portable.backfill.v1\0");
    digest.update(namespace.as_bytes());
    digest.update(b"\0");
    digest.update(stable_legacy_key.as_bytes());
    let digest = digest.finalize();
    let mut random = [0_u8; 10];
    random.copy_from_slice(&digest[..10]);
    uuid_v7_from_parts(timestamp_ms, random)
}

fn uuid_v7_from_parts(timestamp_ms: u64, random: [u8; 10]) -> String {
    let mut bytes = [0u8; 16];
    let timestamp = timestamp_ms.to_be_bytes();
    bytes[..6].copy_from_slice(&timestamp[2..]);
    bytes[6..].copy_from_slice(&random);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid(bytes)
}

fn format_uuid(bytes: [u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_digit() || matches!(*byte, b'a'..=b'f')
        }
    })
}

pub fn is_uuid_v7(value: &str) -> bool {
    is_uuid(value)
        && value.as_bytes().get(14) == Some(&b'7')
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
}

fn parse_utc_rfc3339(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if !value.ends_with('Z') {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
}

trait OptionTimestampExt<T> {
    fn transpose_option(self, error: &str) -> Result<Option<T>, String>;
}

impl<T> OptionTimestampExt<T> for Option<Option<T>> {
    fn transpose_option(self, error: &str) -> Result<Option<T>, String> {
        match self {
            None => Ok(None),
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Err(error.to_string()),
        }
    }
}

/// Serialize JSON with lexicographically sorted object keys and no insignificant
/// whitespace. Arrays retain their order and numbers use serde_json's stable
/// representation.
pub fn canonical_json(value: &Value) -> String {
    let mut output = String::new();
    write_canonical_json(value, &mut output);
    output
}

fn write_canonical_json(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output
            .push_str(&serde_json::to_string(value).expect("string serialization cannot fail")),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output);
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key).expect("object key serialization cannot fail"),
                );
                output.push(':');
                write_canonical_json(&values[key], output);
            }
            output.push('}');
        }
    }
}

pub fn canonical_sha256(value: &Value) -> String {
    let digest = Sha256::digest(canonical_json(value).as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uuid_v7_has_canonical_shape_version_variant_and_timestamp_order() {
        let first = uuid_v7_from_parts(1_700_000_000_000, [0x11; 10]);
        let second = uuid_v7_from_parts(1_700_000_000_001, [0x00; 10]);

        assert!(is_uuid(&first));
        assert_eq!(&first[14..15], "7");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
        assert!(first < second);
    }

    #[test]
    fn canonical_hash_does_not_depend_on_object_insertion_order() {
        let left: Value = serde_json::from_str(r#"{"z":1,"a":{"b":2,"a":1}}"#).unwrap();
        let right: Value = serde_json::from_str(r#"{"a":{"a":1,"b":2},"z":1}"#).unwrap();

        assert_eq!(canonical_json(&left), r#"{"a":{"a":1,"b":2},"z":1}"#);
        assert_eq!(canonical_sha256(&left), canonical_sha256(&right));
    }

    #[test]
    fn record_constructor_hashes_and_validates_content_and_lifecycle() {
        let record = ContextRecordV1::new(
            uuid_v7_from_parts(1, [1; 10]),
            uuid_v7_from_parts(2, [2; 10]),
            "note".to_string(),
            1,
            1,
            uuid_v7_from_parts(3, [3; 10]),
            "2026-08-16T00:00:00Z".to_string(),
            "2026-08-16T00:00:00Z".to_string(),
            None,
            RecordScope {
                scope_id: uuid_v7_from_parts(4, [4; 10]),
                class: ScopeClass::Personal,
            },
            "standard".to_string(),
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: Some("capture".to_string()),
            },
            json!({"title":"Portable", "body":"Preserve me"}),
            json!({"source":"typed"}),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )
        .unwrap();

        assert_eq!(record.contract_version, CONTEXT_RECORD_VERSION);
        assert_eq!(record.content_hash, canonical_sha256(&record.content));
        record.validate().unwrap();
    }

    #[test]
    fn validation_rejects_stale_hash_and_incoherent_lifecycle() {
        let mut record = ContextRecordV1::new(
            uuid_v7_from_parts(1, [1; 10]),
            uuid_v7_from_parts(2, [2; 10]),
            "note".to_string(),
            1,
            1,
            uuid_v7_from_parts(3, [3; 10]),
            "2026-08-16T00:00:00Z".to_string(),
            "2026-08-16T00:00:00Z".to_string(),
            None,
            RecordScope {
                scope_id: uuid_v7_from_parts(4, [4; 10]),
                class: ScopeClass::Personal,
            },
            "standard".to_string(),
            RecordAuthority {
                kind: AuthorityKind::Noted,
                origin: None,
            },
            json!({"body":"first"}),
            json!({}),
            RecordLifecycle {
                state: LifecycleState::Active,
                trashed_at: None,
                tombstoned_at: None,
            },
        )
        .unwrap();

        record.content = json!({"body":"changed"});
        assert!(record.validate().unwrap_err().contains("hash"));
        record.content_hash = canonical_sha256(&record.content);
        record.lifecycle = RecordLifecycle {
            state: LifecycleState::Trash,
            trashed_at: None,
            tombstoned_at: None,
        };
        assert!(record.validate().unwrap_err().contains("trashed_at"));
    }

    #[test]
    fn deterministic_backfill_ids_are_stable_and_namespaced() {
        let first = deterministic_backfill_uuid_v7(1_700_000_000_000, "notes", "42");
        let same = deterministic_backfill_uuid_v7(1_700_000_000_000, "notes", "42");
        let other_table = deterministic_backfill_uuid_v7(1_700_000_000_000, "note_folders", "42");

        assert_eq!(first, same);
        assert_ne!(first, other_table);
        assert!(is_uuid_v7(&first));
        assert!(!is_uuid(&first.to_uppercase()));
    }

    #[test]
    fn unknown_top_level_extensions_survive_round_trip() {
        let raw = format!(
            r#"{{
              "contract_version":"noted.context-record.v1",
              "library_id":"{}",
              "record_id":"{}",
              "kind":"note",
              "record_schema_version":1,
              "revision":1,
              "version_id":"{}",
              "created_at":"2026-08-16T00:00:00Z",
              "updated_at":"2026-08-16T00:00:00Z",
              "scope":{{"scope_id":"{}","class":"personal"}},
              "sensitivity":"standard",
              "authority":{{"kind":"noted"}},
              "content":{{"body":"hello"}},
              "content_hash":"{}",
              "provenance":{{}},
              "lifecycle":{{"state":"active"}},
              "example.vendor/retained":{{"answer":42}}
            }}"#,
            uuid_v7_from_parts(1, [1; 10]),
            uuid_v7_from_parts(2, [2; 10]),
            uuid_v7_from_parts(3, [3; 10]),
            uuid_v7_from_parts(4, [4; 10]),
            canonical_sha256(&json!({"body":"hello"})),
        );
        let record: ContextRecordV1 = serde_json::from_str(&raw).unwrap();
        record.validate().unwrap();
        assert_eq!(
            record.extensions["example.vendor/retained"],
            json!({"answer":42})
        );
        let encoded = serde_json::to_value(&record).unwrap();
        assert_eq!(encoded["example.vendor/retained"], json!({"answer":42}));
    }

    #[test]
    fn unaccepted_local_branch_stays_distinct_from_accepted_record() {
        let branch = LocalBranch {
            branch_id: uuid_v7_from_parts(1, [1; 10]),
            base_revision: 0,
            base_version_id: None,
            local_revision: 3,
            working_version_id: uuid_v7_from_parts(2, [2; 10]),
            content_hash: canonical_sha256(&json!({"body":"third offline edit"})),
            state: LocalBranchState::Pending,
        };
        branch.validate().unwrap();

        let mut invalid = branch.clone();
        invalid.base_version_id = Some(uuid_v7_from_parts(3, [3; 10]));
        assert!(invalid.validate().unwrap_err().contains("unaccepted"));
    }
}
