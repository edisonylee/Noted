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

#[cfg(test)]
mod tests {
    use super::normalize;

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
