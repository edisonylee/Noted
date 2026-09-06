import type { Database } from "bun:sqlite";
import { readFileSync } from "node:fs";

export function initializeSearch(db: Database) {
  // DDL, backfill, and the version marker succeed together or roll back together.
  db.transaction(() => {
    db.exec("CREATE TABLE IF NOT EXISTS team_migrations (id TEXT PRIMARY KEY)");
    if (db.query("SELECT id FROM team_migrations WHERE id=?").get("search-v1")) return;
    db.exec(readFileSync(new URL("./search.sql", import.meta.url), "utf8"));
    db.exec("INSERT INTO chat_messages_fts(chat_messages_fts) VALUES('rebuild'); INSERT INTO notes_fts(notes_fts) VALUES('rebuild')");
    db.query("INSERT INTO team_migrations(id) VALUES(?)").run("search-v1");
  })();
}

// This is keyword search, not a natural-language answer engine. Short terms
// remain searchable, and FTS operators are always treated as literal words.
export function searchExpression(query: string) {
  if (query.length > 256) throw new Error("Search is limited to 256 characters");
  const tokens = query.match(/[\p{L}\p{N}\p{M}]+/gu) ?? [];
  if (tokens.length > 12) throw new Error("Search is limited to 12 words");
  return tokens.map((token, i) => `"${token}"${i === tokens.length - 1 && token.length >= 2 ? "*" : ""}`).join(" AND ");
}

export function snippetParts(value: string, start: string, end: string) {
  const parts: { text: string; match: boolean }[] = [];
  // SQL bounds the excerpt even for unusually long tokens. Drop an incomplete
  // internal delimiter at that boundary and enforce a display character budget.
  value = value.replace(/\u0001[^\u0002]*$/u, "");
  for (const [i, chunk] of value.split(start).entries()) {
    const stop = i ? chunk.indexOf(end) : -1;
    if (stop < 0) parts.push({ text: chunk, match: false });
    else {
      parts.push({ text: chunk.slice(0, stop), match: true });
      parts.push({ text: chunk.slice(stop + end.length), match: false });
    }
  }
  let remaining = 1600;
  const bounded = [];
  for (const part of parts) {
    if (!part.text) continue;
    if (!remaining) { bounded.push({ text: "…", match: false }); break; }
    bounded.push({ text: part.text.slice(0, remaining), match: part.match });
    remaining -= bounded.at(-1)!.text.length;
    if (part.text.length > bounded.at(-1)!.text.length) { bounded.push({ text: "…", match: false }); break; }
  }
  return bounded;
}
