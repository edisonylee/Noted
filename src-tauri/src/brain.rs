// Brain sync: read Obsidian "brain" vaults (markdown + `[[wikilinks]]`) into
// noted's knowledge graph. This module is the PURE part — parsing a file's
// frontmatter, type, links, and noted-managed region, plus the vault walk and
// git helpers. The DB/embedding orchestration lives in lib.rs; storage in db.rs.
//
// The mapping (see PROTOCOL.md / the brain-sync plan):
//   one .md file        -> one `notes` row (origin = "brain:<vault>")
//   the file's subject  -> one "home" entity, typed by its folder/frontmatter
//   each [[wikilink]]   -> a mention of the link target in this note
// Because noted derives graph edges from co-mention within a note, a brain
// note's wikilinks reconstruct the vault's link graph as co-mention edges.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::entities;

/// Markers delimiting the region noted owns inside a brain note. Everything
/// outside is hand-owned (Obsidian); write-back (Phase 2) only ever touches the
/// span between these, so the two writers never clobber each other.
pub const MANAGED_BEGIN: &str = "<!-- noted:begin -->";
pub const MANAGED_END: &str = "<!-- noted:end -->";

// ── Auto-propagation flag ────────────────────────────────────────────────────
// Whether the background timer writes captures back into the vaults + refreshes
// the personal vault. On by default; persisted as a "1"/"0" file in app data so
// the choice survives restarts. Import + embed always run; this gates the WRITES.
static AUTO_PROPAGATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
const AUTO_FILE: &str = "brain_auto";

pub fn auto_propagate() -> bool {
    AUTO_PROPAGATE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn init_auto(dir: &Path) {
    let on = std::fs::read_to_string(dir.join(AUTO_FILE)).map(|s| s.trim() != "0").unwrap_or(true);
    AUTO_PROPAGATE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn set_auto_propagate(dir: &Path, on: bool) {
    AUTO_PROPAGATE.store(on, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::write(dir.join(AUTO_FILE), if on { "1" } else { "0" });
}

/// A brain note parsed into the pieces noted needs. Pure data — no DB ids yet.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedNote {
    pub vault: String,
    pub rel_path: String,        // vault-relative file path (the sync key)
    pub slug: String,            // kebab id: frontmatter `name`, else filename stem
    pub display_name: String,    // human entity name ("Yi", "BARO")
    pub etype: String,           // person | project | decision | reference | doc
    pub status: Option<String>,
    pub event_date: Option<String>, // frontmatter `updated` || `created` (YYYY-MM-DD)
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub wikilinks: Vec<String>,  // target slugs, deduped, self-link removed
    pub managed: Option<String>, // current content of the noted-managed region
    pub hash: String,            // sha256 of the full raw file (change detection)
}

/// First path segment → entity type. The fallback for anything else (top-level
/// notes, `00-inbox/`) is a generic `doc`.
pub fn folder_to_type(rel_path: &str) -> &'static str {
    let first = rel_path.split('/').next().unwrap_or("");
    match first {
        "people" => "person",
        "projects" => "project",
        "decisions" => "decision",
        "references" => "reference",
        _ => "doc",
    }
}

/// Normalize a frontmatter `type` value to our taxonomy; `None` if unrecognized
/// (caller falls back to the folder). Brains use project|decision|person|
/// reference|note; `note` maps to our `doc`.
fn type_from_frontmatter(t: &str) -> Option<&'static str> {
    match t.trim().to_lowercase().as_str() {
        "person" => Some("person"),
        "project" => Some("project"),
        "decision" => Some("decision"),
        "reference" => Some("reference"),
        "note" | "doc" => Some("doc"),
        _ => None,
    }
}

/// Dedup key for a brain entity. People (and self) are normalized globally so the
/// same person unifies across vaults AND daily captures (the interlink). Work
/// artifacts are vault-scoped (`baro:architecture`) so same-named notes in
/// different vaults — e.g. each vault's `references/architecture.md` — don't
/// collide under the global UNIQUE(norm, type).
pub fn vault_norm(vault: &str, etype: &str, name: &str) -> String {
    let n = entities::normalize(name);
    if etype == "person" {
        n
    } else {
        format!("{vault}:{n}")
    }
}

/// Title-case a kebab slug for a display-name fallback ("brian-cho" -> "Brian Cho").
fn humanize(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A concise display name from the frontmatter title: the part before the first
/// dash separator ("Yi — Amazon AI engineer" -> "Yi"). Falls back to the
/// humanized slug when there's no usable title.
fn display_name(title: Option<&str>, slug: &str) -> String {
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        for sep in [" — ", " – ", " - ", ": "] {
            if let Some((head, _)) = t.split_once(sep) {
                let head = head.trim();
                if !head.is_empty() {
                    return head.to_string();
                }
            }
        }
        return t.to_string();
    }
    humanize(slug)
}

/// Split a `---`-fenced YAML frontmatter block off the top of a file. Returns the
/// raw frontmatter lines (without the fences) and the remaining body. No fence →
/// empty frontmatter and the whole input as body.
fn split_frontmatter(raw: &str) -> (&str, &str) {
    let trimmed = raw.strip_prefix('\u{feff}').unwrap_or(raw); // tolerate BOM
    let rest = match trimmed.strip_prefix("---\n").or_else(|| trimmed.strip_prefix("---\r\n")) {
        Some(r) => r,
        None => return ("", raw),
    };
    // Find a closing fence line ("---" on its own line).
    let mut idx = 0;
    for line in rest.split_inclusive('\n') {
        let t = line.trim_end_matches(['\r', '\n']);
        if t == "---" {
            let fm = &rest[..idx];
            let body = &rest[idx + line.len()..];
            return (fm, body);
        }
        idx += line.len();
    }
    ("", raw) // unterminated fence: treat as no frontmatter
}

/// Minimal YAML-ish frontmatter parser. Handles `key: value`, inline arrays
/// `key: [a, b]`, and quoted scalars — enough for the brains' frontmatter
/// (name/title/type/status/tags/aliases). Not a general YAML parser by design
/// (avoids a new crate, matching the repo's keychain/gcal approach).
fn parse_frontmatter(fm: &str) -> std::collections::HashMap<String, Value> {
    let mut map = std::collections::HashMap::new();
    for line in fm.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once(':') else { continue };
        let key = key.trim().to_lowercase();
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        if let Some(inner) = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let arr: Vec<Value> = inner
                .split(',')
                .map(|s| unquote(s.trim()))
                .filter(|s| !s.is_empty())
                .map(Value::String)
                .collect();
            map.insert(key, Value::Array(arr));
        } else {
            map.insert(key, Value::String(unquote(val)));
        }
    }
    map
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s);
    let s = s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).unwrap_or(s);
    s.trim().to_string()
}

fn fm_string(map: &std::collections::HashMap<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty())
}

fn fm_array(map: &std::collections::HashMap<String, Value>, key: &str) -> Vec<String> {
    match map.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).map(String::from).collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// All `[[wikilink]]` targets in a body, normalized to their slug: `[[a|label]]`
/// → `a`, `[[a#heading]]` → `a`. Lowercased, trimmed, deduped, order-preserving.
pub fn extract_wikilinks(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(close) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + close];
                let target = inner.split('|').next().unwrap_or("");
                let target = target.split('#').next().unwrap_or("");
                let slug = target.trim().to_lowercase();
                if !slug.is_empty() && seen.insert(slug.clone()) {
                    out.push(slug);
                }
                i += 2 + close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Extract the content currently inside the noted-managed region, if present.
pub fn extract_managed(body: &str) -> Option<String> {
    let start = body.find(MANAGED_BEGIN)? + MANAGED_BEGIN.len();
    let end = body[start..].find(MANAGED_END)? + start;
    Some(body[start..end].trim().to_string())
}

/// Filename stem of a vault-relative path ("people/brian-cho.md" -> "brian-cho").
pub fn slug_from_path(rel_path: &str) -> String {
    rel_path
        .rsplit('/')
        .next()
        .unwrap_or(rel_path)
        .strip_suffix(".md")
        .unwrap_or(rel_path)
        .to_lowercase()
}

/// Hex sha256 of arbitrary content — the change-detection / echo-suppression key.
pub fn content_hash(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    let mut s = String::with_capacity(64);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Parse one brain file into a `ParsedNote`. `rel_path` is vault-relative.
pub fn parse_note(vault: &str, rel_path: &str, raw: &str) -> ParsedNote {
    let (fm_raw, body) = split_frontmatter(raw);
    let fm = parse_frontmatter(fm_raw);

    let slug = fm_string(&fm, "name").map(|s| s.to_lowercase()).unwrap_or_else(|| slug_from_path(rel_path));
    let title = fm_string(&fm, "title");
    let etype = fm_string(&fm, "type")
        .and_then(|t| type_from_frontmatter(&t))
        .unwrap_or_else(|| folder_to_type(rel_path))
        .to_string();

    let mut wikilinks = extract_wikilinks(body);
    wikilinks.retain(|w| w != &slug); // a note never co-mentions itself

    // Aliases: frontmatter `aliases`, plus the full title when it differs from
    // the concise display name (so "Rishi Vable" can still resolve to kaizen).
    let mut aliases = fm_array(&fm, "aliases");
    let dn = display_name(title.as_deref(), &slug);
    if let Some(t) = &title {
        if t != &dn && !aliases.contains(t) {
            aliases.push(t.clone());
        }
    }

    ParsedNote {
        vault: vault.to_string(),
        rel_path: rel_path.to_string(),
        slug,
        display_name: dn,
        etype,
        status: fm_string(&fm, "status"),
        event_date: fm_string(&fm, "updated").or_else(|| fm_string(&fm, "created")),
        tags: fm_array(&fm, "tags"),
        aliases,
        wikilinks,
        managed: extract_managed(body),
        hash: content_hash(raw),
    }
}

// ── Vault walk + git (fs side; not covered by the pure unit tests) ───────────

/// Markdown files in a vault worth importing: notes under people/projects/
/// decisions/references/00-inbox (recursively) plus loose top-level notes.
/// Skips dotfiles/dotdirs (.git, .obsidian) and `_`-prefixed files (`_index.md`,
/// `_templates/`) — those are vault scaffolding, not entities.
pub fn collect_markdown_files(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    return out;

    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('_') {
                continue; // .git/.obsidian and _index/_templates scaffolding
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push((rel.to_string_lossy().replace('\\', "/"), raw));
                    }
                }
            }
        }
    }
}

// ── Write-back (Phase 2: noted -> Obsidian) ──────────────────────────────────
// noted only ever writes BETWEEN the managed markers; everything else in the
// file is hand-owned. The block holds capture mentions of the note's subject —
// the daily-capture stream feeding the curated brain profile.

/// Render the inner content of a managed region from an entity's capture
/// mentions (date + curated fact/snippet), newest first.
pub fn render_managed_block(captures: &[(String, String)]) -> String {
    let mut s = String::from(
        "_Captured via noted — auto-generated on sync; edits inside this block are overwritten._\n",
    );
    for (date, text) in captures {
        let one = text.replace('\n', " ");
        s.push_str(&format!("\n- {date} — {}", one.trim()));
    }
    s
}

/// Return `raw` with the managed region's inner content set to `inner`. Replaces
/// the span between the markers if present; otherwise appends a fresh fenced
/// block at the end. Hand-written content outside the markers is never touched.
pub fn apply_managed(raw: &str, inner: &str) -> String {
    let block = format!("{MANAGED_BEGIN}\n{}\n{MANAGED_END}", inner.trim());
    match (raw.find(MANAGED_BEGIN), raw.find(MANAGED_END)) {
        (Some(bs), Some(es)) if es > bs => {
            let end = es + MANAGED_END.len();
            let mut out = String::with_capacity(raw.len());
            out.push_str(&raw[..bs]);
            out.push_str(&block);
            out.push_str(&raw[end..]);
            out
        }
        _ => {
            let mut out = raw.trim_end().to_string();
            out.push_str("\n\n");
            out.push_str(&block);
            out.push('\n');
            out
        }
    }
}

/// Kebab a display name into a filename slug ("Brian Cho" -> "brian-cho").
pub fn slugify(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
            prev_dash = false;
        } else if !prev_dash && !s.is_empty() {
            s.push('-');
            prev_dash = true;
        }
    }
    s.trim_matches('-').to_string()
}

/// A fresh personal-vault person note: frontmatter + heading + the managed
/// region holding their captured mentions. Used by export when the file doesn't
/// exist yet; existing files only get their managed region updated.
pub fn render_new_person_file(
    name: &str,
    slug: &str,
    relationship: Option<&str>,
    today: &str,
    inner: &str,
) -> String {
    let rel = relationship
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| format!("_{r}_\n"))
        .unwrap_or_default();
    let header = format!(
        "---\nname: {slug}\ntitle: {name}\ntype: person\ntags: [personal]\ncreated: {today}\nupdated: {today}\n---\n\n# {name}\n{rel}"
    );
    apply_managed(&header, inner)
}

/// True if `root` is inside a git work tree.
pub fn git_is_repo(root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Stage exactly `paths` (vault-relative) and commit them with `message`. Commits
/// ONLY those files, so any unrelated uncommitted edits the user has stay
/// untouched. Returns the new short sha, or None if nothing was committed.
pub fn git_commit_paths(root: &Path, paths: &[String], message: &str) -> Option<String> {
    if paths.is_empty() || !git_is_repo(root) {
        return None;
    }
    let mut add = Command::new("git");
    add.arg("-C").arg(root).arg("add").arg("--");
    for p in paths {
        add.arg(p);
    }
    if !add.status().ok()?.success() {
        return None;
    }
    let committed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["commit", "-m", message, "--"])
        .args(paths)
        .output()
        .ok()?;
    if !committed.status.success() {
        return None; // e.g. nothing staged changed
    }
    let sha = Command::new("git").arg("-C").arg(root).args(["rev-parse", "--short", "HEAD"]).output().ok()?;
    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Current git HEAD sha of a vault (recorded as the sync checkpoint). None if the
/// path isn't a git repo or git is unavailable.
pub fn git_head(root: &Path) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(["rev-parse", "HEAD"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Default brain vault roots under `~/Brain`, paired with their vault name —
/// used to auto-register on first launch.
pub fn default_vault_roots() -> Vec<(String, PathBuf)> {
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return Vec::new(),
    };
    let brain = home.join("Brain");
    ["baro", "profound", "personal"]
        .iter()
        .map(|v| (v.to_string(), brain.join(v)))
        .filter(|(_, p)| p.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const YI: &str = "---\nname: yi\ntitle: Yi — Amazon AI agent engineer (peer sounding board)\ntype: person\nstatus: active\ntags: [baro, peer, architecture]\ncreated: 2026-06-20\n---\n\n**AI agent engineer at Amazon**. See [[hitl-two-risk-classes]] and\n[[raw-vs-serving-feature-store]]. Also [[yi]] self-link should drop.\n";

    #[test]
    fn parses_person_note() {
        let n = parse_note("baro", "people/yi.md", YI);
        assert_eq!(n.slug, "yi");
        assert_eq!(n.etype, "person");
        assert_eq!(n.display_name, "Yi"); // title trimmed at the em dash
        assert_eq!(n.status.as_deref(), Some("active"));
        assert_eq!(n.tags, vec!["baro", "peer", "architecture"]);
        // wikilinks: deduped, self-link removed
        assert_eq!(n.wikilinks, vec!["hitl-two-risk-classes", "raw-vs-serving-feature-store"]);
        // the long title is kept as an alias for resolution
        assert!(n.aliases.iter().any(|a| a.contains("Amazon AI agent engineer")));
    }

    #[test]
    fn folder_maps_to_type() {
        assert_eq!(folder_to_type("people/yi.md"), "person");
        assert_eq!(folder_to_type("projects/baro.md"), "project");
        assert_eq!(folder_to_type("projects/orchestration/prd.md"), "project");
        assert_eq!(folder_to_type("decisions/hitl.md"), "decision");
        assert_eq!(folder_to_type("references/architecture.md"), "reference");
        assert_eq!(folder_to_type("00-inbox/scratch.md"), "doc");
    }

    #[test]
    fn vault_norm_scopes_artifacts_but_not_people() {
        // people unify globally (interlink across vaults + captures)
        assert_eq!(vault_norm("baro", "person", "Yi"), "yi");
        assert_eq!(vault_norm("profound", "person", "yi"), "yi");
        // references collide across vaults unless scoped
        assert_eq!(vault_norm("baro", "reference", "Architecture"), "baro:architecture");
        assert_eq!(vault_norm("profound", "reference", "architecture"), "profound:architecture");
        assert_ne!(
            vault_norm("baro", "reference", "architecture"),
            vault_norm("profound", "reference", "architecture")
        );
    }

    #[test]
    fn wikilink_variants() {
        let links = extract_wikilinks("[[plain]] [[target|label]] [[note#heading]] [[plain]] text");
        assert_eq!(links, vec!["plain", "target", "note"]); // alias/heading stripped, deduped
    }

    #[test]
    fn frontmatter_type_overrides_folder() {
        // a note living in references/ but typed person in frontmatter -> person
        let raw = "---\nname: edison\ntype: person\n---\nbody";
        let n = parse_note("baro", "references/edison.md", raw);
        assert_eq!(n.etype, "person");
        assert_eq!(n.display_name, "Edison"); // humanized slug (no title)
    }

    #[test]
    fn no_frontmatter_is_doc() {
        let n = parse_note("personal", "00-inbox/idea.md", "just a thought with [[a-link]]");
        assert_eq!(n.etype, "doc");
        assert_eq!(n.slug, "idea");
        assert_eq!(n.wikilinks, vec!["a-link"]);
        assert!(n.tags.is_empty());
    }

    #[test]
    fn extracts_managed_region() {
        let body = format!("hand-written\n{MANAGED_BEGIN}\ncaptured stuff\n{MANAGED_END}\nmore");
        assert_eq!(extract_managed(&body).as_deref(), Some("captured stuff"));
        assert_eq!(extract_managed("no region here"), None);
    }

    #[test]
    fn apply_managed_replaces_in_place_and_preserves_handwritten() {
        let raw = format!(
            "# Yi\nHand-written profile.\n\n{MANAGED_BEGIN}\nold\n{MANAGED_END}\n\n## Footer kept"
        );
        let out = apply_managed(&raw, "new line");
        assert!(out.contains("Hand-written profile."));
        assert!(out.contains("## Footer kept"));
        assert!(out.contains("new line"));
        assert!(!out.contains("old"));
        assert_eq!(extract_managed(&out).as_deref(), Some("new line"));
        // exactly one managed region after a rewrite
        assert_eq!(out.matches(MANAGED_BEGIN).count(), 1);
    }

    #[test]
    fn apply_managed_appends_when_absent() {
        let out = apply_managed("# Note\nbody", "captured");
        assert!(out.starts_with("# Note\nbody"));
        assert_eq!(extract_managed(&out).as_deref(), Some("captured"));
    }

    #[test]
    fn render_block_lists_captures_newest_first() {
        let block = render_managed_block(&[
            ("2026-06-22".into(), "talked to Yi about feature stores".into()),
            ("2026-06-20".into(), "lunch".into()),
        ]);
        assert!(block.contains("- 2026-06-22 — talked to Yi about feature stores"));
        assert!(block.contains("- 2026-06-20 — lunch"));
        // round-trips through apply/extract unchanged
        let raw = apply_managed("body", &block);
        assert_eq!(extract_managed(&raw).as_deref(), Some(block.trim()));
    }

    #[test]
    fn slugify_kebabs_names() {
        assert_eq!(slugify("Brian Cho"), "brian-cho");
        assert_eq!(slugify("  Dr. Smith  "), "dr-smith");
        assert_eq!(slugify("José—Luis"), "jos-luis");
    }

    #[test]
    fn new_person_file_has_frontmatter_and_managed_block() {
        let inner = render_managed_block(&[("2026-06-22".into(), "coffee".into())]);
        let f = render_new_person_file("Brian Cho", "brian-cho", Some("CEO"), "2026-06-23", &inner);
        // round-trips: parses back to a person note with the captured mention
        let p = parse_note("personal", "people/brian-cho.md", &f);
        assert_eq!(p.etype, "person");
        assert_eq!(p.slug, "brian-cho");
        assert_eq!(p.display_name, "Brian Cho");
        assert!(f.contains("_CEO_"));
        assert!(extract_managed(&f).unwrap().contains("2026-06-22 — coffee"));
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
        assert_eq!(content_hash("abc").len(), 64);
    }
}
