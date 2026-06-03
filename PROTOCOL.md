# noted — Extraction Protocol

> How a single captured note (typed or photographed) is split, routed, and
> extracted into one or more category entries. This is the contract between your
> writing, the deterministic parser, and the local LLM.

Status: **design / proposed.** Not yet implemented. Supersedes the current
one-note-one-category behavior in `src-tauri/src/pipeline.rs`.

---

## 1. Goals

1. **One note → many categories.** A diary page that covers your schedule, your
   gym session, and what you ate should produce *three* entries, not one.
2. **You control routing when you want to.** Writing a `Section header:` makes
   that section's category **deterministic** — decided by code, not guessed by
   the model. This is the main defense against misclassification.
3. **Bounded categories.** Untagged content is classified conservatively. A
   reserved `misc` category absorbs anything that doesn't clearly belong, so the
   model never invents a junk category just to avoid leaving content homeless.
4. **No new infrastructure.** No MCP, no servers. This is a prompt + schema
   contract plus a small Rust pre-parser. The LLM stays where it is (Ollama).

---

## 2. Section-header syntax

A **section header** is a line whose sole job is to open a category section.
Everything beneath it — until the next header or end of note — is that section's
body. Content may also sit on the header line itself, after the separator.

### Recognized forms
A line is a header when, trimmed, it is a short label (1–3 words) followed by a
separator and nothing else meaningful before the separator:

```
Food:                     ← colon
Gym —                     ← em dash
Schedule -                ← hyphen
## Work                   ← markdown heading (optional support)
Food: rice, chicken, rice ← header + inline body on the same line
```

### Grammar (informal)
```
header      := label sep
label       := 1–3 words, letters/spaces only
sep         := ":" | "—" | " - " | leading "#"/"##"
body        := the rest of that line (if any) + all following lines
               up to the next header or EOF
```

### Stoplist (NOT headers)
To avoid false positives, these labels are ignored as routing headers and treated
as ordinary text: `note`, `notes`, `todo`, `ps`, `today`, `tomorrow`,
`yesterday`, `am`, `pm`, `re`, `update`. (Extendable.)

### Worked example
Input:
```
6/2

Schedule:
9-12 work on noted
2-4 class

Gym —
squat 245x5x3
bench 185x5

Food: oatmeal, chicken bowl, protein shake

felt a bit tired today
```
Parses to four segments:
| Hint (header) | Body |
|---|---|
| `schedule` | "9-12 work on noted / 2-4 class" |
| `gym` | "squat 245x5x3 / bench 185x5" |
| `food` | "oatmeal, chicken bowl, protein shake" |
| *(none)* | "felt a bit tired today" |

The leading `6/2` is the note date (handled by the existing
`extract_date_from_text`), not a section.

---

## 3. Routing rules

Each segment is routed by its hint, deterministically where possible:

| Segment hint | Routing decision |
|---|---|
| Header matches an existing category (after `snap_category`) | **Route there. Category fixed by code.** LLM only extracts `data`. |
| Header matches a category **synonym/keyword** | Same — snap to the canonical category. |
| Header is a new label, not in the catalog | **Create it** (a header is an explicit tag, so creation is allowed under the conservative policy). LLM extracts `data`. |
| No header (untagged segment) | Run the **normal classifier**. It may land on an existing category or `misc`. New categories are **not** created from untagged text unless content is substantial (see §5). |

> The key property: for any segment you bothered to tag, the model is **never
> asked to choose the category** — it only fills in the structured data. That is
> what removes most classification mistakes.

---

## 4. Schema changes

### Before (current — single proposal)
```json
{ "category": string, "is_new_category": bool, "description": string,
  "event_date": string|null, "data": object }
```

### After (multi-entry)
One note yields one envelope with a shared date and an array of entries:
```json
{
  "raw_text": string,          // transcription (photo path) or original text
  "event_date": string|null,   // one calendar day for the whole note
  "entries": [
    {
      "category": string,
      "is_new_category": bool,
      "description": string,    // only meaningful when is_new_category
      "routed_by": "header" | "classifier",   // provenance, for the UI + trust
      "data": object
    }
    // ...one per segment
  ]
}
```

`validate_proposal`, `snap_category`, and `resolve_date` stay almost as-is — they
just run **per entry** inside a loop instead of once. `is_new_category` remains
decided authoritatively from `known_names`, never trusted from the model.

---

## 5. Category guardrails

1. **Reserved `misc`.** Always exists. The fallback for untagged content with no
   confident home. The classifier is told: *prefer `misc` over inventing a
   category you're unsure about.*
2. **Conservative creation.** A new category is created only when:
   - the user opened it with an explicit header (§3), **or**
   - an untagged segment has *substantial* content for a coherent new topic
     (more than a passing mention — heuristic: ≥ N tokens and a repeated/structured
     pattern). Otherwise → `misc`.
3. **Keyword anchors.** Each category carries a small synonym list
   (`gym` → squat, bench, reps, sets; `food` → ate, meal, calories, breakfast).
   These feed the catalog so the model snaps to existing categories instead of
   minting near-duplicates (`meals` vs `food` vs `diet`), and let the Rust router
   pre-match headers to canonical names.

---

## 6. Pipeline flow

### Typed note (`categorize`)
```
text
 └─ extract_date_from_text ............... note date
 └─ split_sections(text) ................. Vec<Segment{ hint, body }>   [Rust, deterministic]
 └─ for each segment:
      hint present → fixed category + extract data        [1 LLM extract call, batched]
      no hint      → classify + extract                   (may be misc)
 └─ assemble { raw_text, event_date, entries[] }
```

### Photo note (`categorize_photo`)
```
image
 └─ VISION model: transcribe → raw_text                   [1 vision call]
 └─ (same as typed note, on raw_text)
```
Vision still does **transcription only** for routing purposes; the deterministic
splitter runs on the transcription, so headers route the same whether you typed
or photographed the page.

### Batching the LLM calls
To avoid N round-trips: send **one** extraction request containing all segments
pre-labeled with their fixed categories, asking the model to return `entries[]`
with those categories held constant and `data` filled per segment. Untagged
segments are the only ones it classifies. (Tradeoff noted in §8.)

---

## 7. Review UI (human-in-the-loop)

The review step becomes a **stack of N cards**, one per entry, each with its
existing controls (category name, new/existing badge, date, editable `data`
JSON). Per card you can:
- approve / edit / **discard** the entry,
- **re-route** it to a different category (override the model),
- see a `routed_by` indicator (`header` = you tagged it, `classifier` = model
  guessed) so you know which entries to scrutinize.

Commit writes each approved entry as its own note+entry row, reusing the existing
`commit` path in a loop. New categories are created once, then shared.

---

## 8. Tradeoffs & open questions

- **One batched call vs per-section calls.** Batched = fewer round-trips, but a
  weak 7B model may blur sections. Per-section = slower but rock-solid isolation.
  Proposed: **batched**, with a fallback to per-section if validation fails.
- **Header false positives.** The stoplist + "label must be ≤3 words, letters
  only" should cover most cases. Worst case the user sees a stray card and
  discards it in review — non-destructive.
- **Per-entry dates.** For now `event_date` is shared for the whole note. If you
  ever paste multi-day logs, we'd lift date to the entry level. Out of scope v1.
- **Synonym lists — who maintains them?** Start with a seed per category; let the
  model's accepted routings grow them over time, or edit by hand. TBD.
- **`misc` review noise.** If `misc` fills up, that's a signal to promote a
  recurring sub-topic into its own category. Could surface that as a hint later.

---

## 9. Implementation surface (when we build)

| File | Change |
|---|---|
| `src-tauri/src/pipeline.rs` | `split_sections()` parser; loop `validate`/`snap` per entry; new `entries[]` assembly; update both prompt builders to describe headers + `misc` + `entries[]`. |
| `src-tauri/src/lib.rs` | `categorize_note` / `categorize_photo` return the envelope; `commit` loops over approved entries. |
| `src-tauri/src/db.rs` | ensure reserved `misc`; optional `keywords` per category in the catalog. |
| `src/api.ts` | types: proposal → envelope with `entries[]`. |
| Review component (`App.tsx`) | render N cards; per-card approve/edit/discard/re-route. |

No changes to transport, no MCP, no new dependencies.
