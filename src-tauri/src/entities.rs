// Knowledge-graph entity resolution (Phase 2). Normalize a candidate name, then
// resolve it against existing entities: exact/alias match, else embedding
// nearest-neighbor (a merge suggestion the user confirms), else new. Storage
// lives in db.rs; this module orchestrates normalization + embedding.

use anyhow::Result;
use rusqlite::Connection;

use crate::db;
use crate::ollama;

/// Normalize a raw entity name into a dedup key: lowercase, trim, collapse inner
/// whitespace, drop a leading article ("The Gym" -> "gym").
pub fn normalize(name: &str) -> String {
    let n = name.trim().to_lowercase();
    let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    for art in ["the ", "a ", "an "] {
        if let Some(rest) = n.strip_prefix(art) {
            return rest.to_string();
        }
    }
    n
}

/// Text embedded for an entity (used for both resolution and storage, so the
/// nearest-neighbor query matches how entities were indexed).
pub fn embed_text(name: &str, etype: &str) -> String {
    format!("{name} ({etype})")
}

/// L2-normalize so vec0's default L2 distance ranks like cosine.
fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

/// Outcome of resolving a candidate entity against the store.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resolution {
    Exact(i64),        // matched an existing entity (norm/type or alias)
    Suggest(i64, f32), // a near neighbor — propose a merge; user confirms
    New,               // no confident match; create on confirm
}

/// Above this cosine similarity a differently-spelled name is proposed as the
/// same entity (a merge the user confirms in review).
const SIM_THRESHOLD: f32 = 0.86;

/// Resolve using a precomputed embedding. Pure sync — safe to call while holding
/// the DB lock (do the embedding off-lock first, then call this).
pub fn resolve_with_embedding(
    conn: &Connection,
    name: &str,
    etype: &str,
    emb: &[f32],
) -> Result<Resolution> {
    let norm = normalize(name);
    if let Some(id) = db::entity_exact(conn, &norm, etype)? {
        return Ok(Resolution::Exact(id));
    }
    if let Some((id, dist)) = db::nearest_entity(conn, emb, etype)? {
        // unit vectors: L2^2 = 2(1 - cos)  ->  cos = 1 - dist^2 / 2
        let sim = 1.0 - dist * dist / 2.0;
        if sim >= SIM_THRESHOLD {
            return Ok(Resolution::Suggest(id, sim));
        }
    }
    Ok(Resolution::New)
}

/// Resolve a candidate (name, type): exact norm/alias match -> Exact; else the
/// nearest same-type entity by embedding above threshold -> Suggest; else New.
pub async fn resolve(conn: &Connection, name: &str, etype: &str) -> Result<Resolution> {
    let emb = embed_entity(name, etype).await?;
    resolve_with_embedding(conn, name, etype, &emb)
}

/// Embed an entity name for storage (unit vector), matching `resolve`'s query.
pub async fn embed_entity(name: &str, etype: &str) -> Result<Vec<f32>> {
    Ok(unit(ollama::embed(&embed_text(name, etype)).await?))
}

// ── Deterministic people-from-fields promotion ──────────────────────────────
// The model sometimes files a person into an entry field (e.g. {"with": "Khai"})
// but forgets to also list them in the `entities` array, so they never become a
// person in the graph. As a safety net we scan the saved `data` for person-ish
// fields and promote those names ourselves, funneling them through the SAME
// resolve→create→mention path as model-surfaced entities.

/// Field keys whose values name a person. Deliberately conservative — a generic
/// `name` key labels exercises/foods/tasks, so it's excluded.
const PERSON_KEYS: &[&str] = &[
    "with",
    "who",
    "people",
    "person",
    "attendees",
    "attendee",
    "friend",
    "friends",
    "coworker",
    "coworkers",
    "colleague",
    "colleagues",
];

/// Values that are never a real person (the narrator, groups, pronouns).
const PERSON_STOPLIST: &[&str] = &[
    "me", "myself", "i", "everyone", "team", "them", "people", "us", "we", "they",
];

/// True if `s` looks like a plausible person name worth promoting.
fn looks_like_person(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.len() > 60 {
        return false;
    }
    if t.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    !PERSON_STOPLIST.contains(&t.to_lowercase().as_str())
}

/// Collect candidate person names from one JSON value found under a person key.
/// Handles a bare string, an array of strings, and an array of {"name": ...}.
fn collect_person_values(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => {
            if looks_like_person(s) {
                out.push(s.trim().to_string());
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                match item {
                    serde_json::Value::String(s) if looks_like_person(s) => {
                        out.push(s.trim().to_string());
                    }
                    serde_json::Value::Object(o) => {
                        if let Some(serde_json::Value::String(s)) = o.get("name") {
                            if looks_like_person(s) {
                                out.push(s.trim().to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Walk an entry's `data` JSON and return the names of people it references via
/// person-ish field keys (deduped, order-preserving). Used to promote people the
/// model put in a field but omitted from the `entities` array.
pub fn person_names_from_data(data: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk(data, &mut out);
    // dedupe by normalized form, keep first-seen spelling
    let mut seen = std::collections::HashSet::new();
    out.retain(|n| seen.insert(normalize(n)));
    return out;

    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    if PERSON_KEYS.contains(&k.to_lowercase().as_str()) {
                        collect_person_values(val, out);
                    }
                    walk(val, out);
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    walk(item, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize, person_names_from_data};
    use serde_json::json;

    #[test]
    fn promotes_person_from_with_field() {
        // The exact shape from the user's note that went missing.
        let data =
            json!({"conversations": [{"topic": ["plans to go to New York"], "with": "Khai"}]});
        assert_eq!(person_names_from_data(&data), vec!["Khai".to_string()]);
    }

    #[test]
    fn handles_strings_arrays_and_name_objects() {
        assert_eq!(
            person_names_from_data(&json!({"who": ["Sam", "Alex"]})),
            vec!["Sam".to_string(), "Alex".to_string()]
        );
        assert_eq!(
            person_names_from_data(&json!({"attendees": [{"name": "Dana"}, {"name": "Lee"}]})),
            vec!["Dana".to_string(), "Lee".to_string()]
        );
    }

    #[test]
    fn rejects_non_people_and_dedupes() {
        // digits, stoplist, and the generic `name` key are all ignored.
        let data = json!({
            "with": "me",
            "exercises": [{"name": "Squat", "weight": 245}],
            "count": 3,
        });
        assert!(person_names_from_data(&data).is_empty());
        // dedupe by normalized form (case-insensitive): "Kai" and "kai" collapse to one
        let deduped = person_names_from_data(&json!({"with": "Kai", "people": ["kai"]}));
        assert_eq!(deduped.len(), 1);
        assert_eq!(normalize(&deduped[0]), "kai");
    }

    #[test]
    fn normalizes_articles_case_and_space() {
        assert_eq!(normalize("The Gym"), "gym");
        assert_eq!(normalize("  Planet   Fitness "), "planet fitness");
        assert_eq!(normalize("Dr. Smith"), "dr. smith");
        assert_eq!(normalize("a chipotle bowl"), "chipotle bowl");
    }

    #[test]
    fn people_group_by_case_not_by_full_name() {
        // the grouping contract the People view relies on
        assert_eq!(normalize("Mike"), normalize("mike"));
        assert_ne!(normalize("Sarah"), normalize("Sarah Chen"));
    }
}
