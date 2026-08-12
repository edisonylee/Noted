# Noted Library format v1

Status: proposed snapshot contract; implementation begins after the storage
foundation is safe.

A Noted Library is a manual, point-in-time disclosure/portability snapshot. It is
not watched, bidirectional, or authoritative while open on disk.

## Layout

~~~text
noted-library-v1/
  manifest.json
  records/
    {record_uuid}.json
  content/
    {record_uuid}.md
  attachments/                 # absent unless explicitly included
  receipts/
    export.json
~~~

`manifest.json` includes format version, library ID, export ID, creation instant,
producer version, selected scopes/kinds/date range, record count, byte count,
attachment policy, and SHA-256 for every file. Paths are relative and must not
contain host usernames or original absolute paths.

Each record JSON is a `ContextRecordV1` envelope. Markdown is a human-readable
lossless rendering whose front matter repeats only portable identity, revision,
time, scope, sensitivity, and provenance fields. Import trusts the signed/hashed
manifest and record JSON, not editable Markdown front matter alone.

## Defaults and exclusions

- User selects scopes and record kinds before creation.
- Attachments, retained audio/video, voice templates, secrets, tokens, provider
  configuration, caches, vectors, graph projections, and query logs are excluded.
- Generated but unapproved artifacts are excluded.
- Unknown-scope or restricted records are excluded until explicitly resolved.
- The UI warns that the output is plaintext-sensitive and may disclose many
  records at once.

Export writes into a private staging directory, validates hashes and counts, then
atomically hands the completed directory/archive to the user. Cancellation removes
Noted-controlled staging data. Once handed off, later revocation or deletion in
Noted cannot remove the user's copy.

## Import

Import validates version, paths, hashes, IDs, revisions, and size ceilings before
writing. Import into the same library preserves identity. Import into a different
library records the source library/record IDs as provenance, deduplicates exact
hashes, and assigns a new local ID for an identity/content conflict. Import never
silently overwrites an external authoritative file.

