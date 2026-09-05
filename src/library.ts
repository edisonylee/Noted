import type { NoteRow } from "./api";

export function isDocumentNote(note: Pick<NoteRow, "note_kind">): boolean {
  return note.note_kind === "document";
}
