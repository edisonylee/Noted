import { useEffect, useState } from "react";
import { FileText, Search } from "lucide-react";
import { api, type NoteRow } from "../api";
import { isDocumentNote } from "../library";
import { TeamDialog } from "./TeamDialog";
import { team, orgPath } from "./client";
import { shortTime } from "./messaging";
import {
  findSharedDocument,
  PublishDocument,
  type LocalDocument,
} from "./PublishDocument";
import type { PendingMeeting } from "./MeetingPicker";
import type { TeamNote, TeamSpace } from "./types";

// The picker starts from documents that live only on this Mac, so the rows
// come from the local Library, not the team. A document already shared in
// this team is staged at once; one that is not goes through the publish sheet
// first and is staged from the note it produces.
export function DocumentPicker({
  org,
  roomId,
  spaces,
  onClose,
  onChoose,
}: {
  org: string;
  roomId: string;
  spaces: TeamSpace[];
  onClose: () => void;
  onChoose: (reference: PendingMeeting) => void;
}) {
  const [documents, setDocuments] = useState<NoteRow[]>([]);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("");
  const [resolving, setResolving] = useState<number | null>(null);
  const [publishing, setPublishing] = useState<LocalDocument | null>(null);
  useEffect(() => {
    let active = true;
    setBusy(true);
    setError("");
    api
      .listNotes()
      .then((notes) => {
        if (!active) return;
        setDocuments(
          notes
            .filter((note) => isDocumentNote(note) && !note.trashed_at)
            .sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
        );
      })
      .catch((e) => {
        if (active) setError(String(e));
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, [retry]);
  // Same rhythm as the meeting picker so typing feels identical in both.
  useEffect(() => {
    const timer = window.setTimeout(
      () => setFilter(query.trim().toLowerCase()),
      query ? 180 : 0,
    );
    return () => window.clearTimeout(timer);
  }, [query]);
  const rows = filter
    ? documents.filter((note) => note.title.toLowerCase().startsWith(filter))
    : documents;
  const stage = (note: TeamNote) =>
    onChoose({
      id: note.id,
      revision: note.revision,
      title: note.title,
      occurred_at: note.occurred_at,
      collection:
        spaces.find((space) => space.id === note.space_id)?.name ?? "",
      kind: "document",
    });
  const choose = async (note: NoteRow) => {
    setResolving(note.id);
    setError("");
    try {
      const shared = await findSharedDocument(org, note.id);
      if (shared) {
        const targets = await team.request<{ id: string }[]>(
          "GET",
          orgPath(org, `/notes/${shared.id}/share-targets`),
        );
        if (!targets.some((target) => target.id === roomId))
          throw new Error(
            "Everyone in this conversation needs access to this document before it can be referenced.",
          );
        stage(shared);
      } else
        setPublishing({
          id: note.id,
          title: note.title,
          documentJson: note.document_json,
          updatedAt: note.updated_at,
        });
    } catch (e) {
      setError(String(e));
    } finally {
      setResolving(null);
    }
  };
  if (publishing)
    return (
      <PublishDocument
        document={publishing}
        preferredOrg={org}
        lockOrg
        roomId={roomId}
        onClose={() => setPublishing(null)}
        onPublished={stage}
      />
    );
  return (
    <TeamDialog
      title="Share a document"
      onClose={onClose}
      busy={resolving !== null}
      className="meeting-picker"
    >
      <p className="team-muted">
        Choose a document from your Library. One that is not shared with this
        team yet is published first; your message stays a draft until you send
        it.
      </p>
      <label className="meeting-picker-search">
        <Search size={16} aria-hidden="true" />
        <input
          aria-label="Search your documents"
          placeholder="Search documents by title"
          maxLength={200}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      <div className="meeting-picker-results" aria-busy={busy}>
        {rows.map((note) => (
          <button
            type="button"
            key={note.id}
            disabled={busy || resolving !== null}
            aria-busy={resolving === note.id || undefined}
            onClick={() => void choose(note)}
          >
            <FileText size={18} aria-hidden="true" />
            <span>
              <strong>{note.title || "Untitled document"}</strong>
              <small>Edited {shortTime(note.updated_at)}</small>
            </span>
          </button>
        ))}
        {busy && <p role="status">Loading your documents…</p>}
        {error && (
          <p role="alert" className="team-error">
            {error}{" "}
            <button
              type="button"
              className="team-text-button"
              onClick={() => setRetry((n) => n + 1)}
            >
              Try again
            </button>
          </p>
        )}
        {!busy && !error && !rows.length && (
          <p className="team-muted">
            {filter
              ? "No matching documents."
              : "No documents in your Library yet"}
          </p>
        )}
      </div>
    </TeamDialog>
  );
}
