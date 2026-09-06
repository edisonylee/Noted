import { useEffect, useMemo, useState } from "react";
import { Check, Lock, Users } from "lucide-react";
import { api } from "../api";
import { MdBlock } from "../MeetingMarkdownView";
import {
  isStructuredDocument,
  plainTextToDocument,
  type StructuredDocument,
} from "../editor/document";
import { documentToMarkdown } from "../editor/documentMarkdown";
import { team, orgPath } from "./client";
import { TeamDialog } from "./TeamDialog";
import { collectionName } from "./presentation";
import type { TeamNote, TeamNoteRow, TeamOrg, TeamSnapshot } from "./types";
import "./teams.css";

// The team's notes field; the server rejects anything longer, so the sheet
// refuses before a byte leaves the Mac.
const MARKDOWN_LIMIT = 300_000;

export type LocalDocument = {
  id: number;
  title: string;
  documentJson: string | null;
  updatedAt: string;
};

// Resolve only opaque identities minted in this vault, in the current account.
// Numeric legacy keys remain ordinary shared notes; never guess their origin.
export async function findSharedDocument(
  org: string,
  id: number,
): Promise<TeamNote | null> {
  const snapshot = await team.request<TeamSnapshot>("GET", orgPath(org));
  if (!snapshot.document_publication_review)
    throw new Error("Update the team server before sharing local documents.");
  const key = await api.teamDocumentIdentity(id);
  const params = new URLSearchParams({ kind: "document", source_key: key });
  const rows = await team.request<TeamNoteRow[]>(
    "GET",
    orgPath(org, `/notes?${params}`),
  );
  const matches = rows.filter(
    (row) =>
      row.source_key === key &&
      row.owner_id === snapshot.user.id &&
      row.kind === "document",
  );
  if (matches.length > 1)
    throw new Error(
      "Multiple shared copies found. Open the intended copy in Team to edit it; automatic replacement is disabled.",
    );
  if (!matches.length) return null;
  const note = await team.request<TeamNote>(
    "GET",
    orgPath(org, `/notes/${matches[0].id}`),
  );
  if (
    note.source_key !== key ||
    note.owner_id !== snapshot.user.id ||
    note.kind !== "document"
  )
    throw new Error(
      "The shared document identity changed. Review the destination again.",
    );
  return note;
}

function parseDocument(json: string | null): StructuredDocument | null {
  if (!json) return null;
  try {
    const value: unknown = JSON.parse(json);
    return isStructuredDocument(value) ? value : null;
  } catch {
    return null;
  }
}

export function PublishDocument({
  document,
  preferredOrg,
  lockOrg = false,
  roomId,
  onClose,
  onPublished,
}: {
  document: LocalDocument;
  preferredOrg?: string;
  // The composer stages the result into one conversation, so the team it
  // belongs to is fixed: a switch here would publish into another org and
  // stage a foreign note id into this room.
  lockOrg?: boolean;
  roomId?: string;
  onClose: () => void;
  onPublished: (note: TeamNote) => void;
}) {
  const [orgs, setOrgs] = useState<TeamOrg[]>([]),
    [org, setOrg] = useState("");
  const [data, setData] = useState<TeamSnapshot | null>(null),
    [space, setSpace] = useState("");
  const [review, setReview] = useState(0);
  const [eligibleSpaces, setEligibleSpaces] = useState<string[] | null>(null);
  const [folders, setFolders] = useState<string[]>([]);
  const [title, setTitle] = useState(document.title.trim());
  // undefined while the lookup runs: the primary must not read "Share" and
  // then create a duplicate a moment before the existing copy is found.
  const [existing, setExisting] = useState<TeamNote | null | undefined>(
    undefined,
  );
  const [rawText, setRawText] = useState<string | null>(null);
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [result, setResult] = useState<TeamNote | null>(null);
  useEffect(() => {
    let active = true;
    team
      .request<TeamOrg[]>("GET", "/v1/orgs")
      .then((values) => {
        if (active) {
          setOrgs(values);
          setOrg(
            values.find((o) => o.id === preferredOrg)?.id ??
              values[0]?.id ??
              "",
          );
        }
      })
      .catch(() => {
        if (active)
          setError(
            "Connect to a team from Team in the sidebar before sharing a document.",
          );
      });
    return () => {
      active = false;
    };
  }, [preferredOrg]);
  useEffect(() => {
    let active = true;
    setData(null);
    setError("");
    setEligibleSpaces(null);
    setSpace("");
    setFolders([]);
    setExisting(org ? undefined : null);
    if (org) {
      if (roomId)
        team
          .request<string[]>(
            "GET",
            orgPath(org, `/chat-rooms/${roomId}/document-destinations`),
          )
          .then((ids) => {
            if (active) setEligibleSpaces(ids);
          })
          .catch((e) => {
            if (active) setError(String(e));
          });
      team
        .request<TeamSnapshot>("GET", orgPath(org))
        .then((value) => {
          if (active) {
            setData(value);
            if (!value.document_publication_review)
              setError(
                "Update the team server before sharing local documents.",
              );
          }
        })
        .catch((e) => {
          if (active) setError(String(e));
        });
      findSharedDocument(org, document.id)
        .then((note) => {
          if (!active) return;
          setExisting(note);
          if (note) {
            setSpace(note.space_id);
            setFolders(note.folder_ids);
          }
        })
        .catch((e) => {
          if (active) {
            setExisting(undefined);
            setError(String(e));
          }
        });
    }
    return () => {
      active = false;
    };
  }, [org, document.id, roomId, review]);
  const parsed = useMemo(
    () => parseDocument(document.documentJson),
    [document.documentJson],
  );
  // A note without a usable rich body is shown by the editor as its plain
  // text, one paragraph per line; the shared copy should read the same way.
  useEffect(() => {
    if (parsed) return;
    let active = true;
    api
      .listNotes()
      .then((rows) => {
        if (active)
          setRawText(rows.find((r) => r.id === document.id)?.raw_text ?? "");
      })
      .catch((e) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, [parsed, document.id]);
  const exported = useMemo(() => {
    const source =
      parsed ?? (rawText == null ? null : plainTextToDocument(rawText));
    return source ? documentToMarkdown(source) : null;
  }, [parsed, rawText]);
  const destination = data?.spaces.find((s) => s.id === space);
  const length = exported?.markdown.length ?? 0,
    tooLong = length > MARKDOWN_LIMIT,
    empty = !!exported && !exported.markdown.trim();
  const ready =
    !!org &&
    !!space &&
    !!data?.document_publication_review &&
    !error &&
    (!roomId || !!eligibleSpaces?.includes(space)) &&
    !!exported &&
    existing !== undefined &&
    !!title.trim() &&
    !empty &&
    !tooLong &&
    (!existing || existing.can_edit);
  const publish = async () => {
    if (!ready || busy || !exported || !data) return;
    setBusy(true);
    setError("");
    try {
      const sourceKey = await api.teamDocumentIdentity(document.id);
      const note = await api.teamPublishDocument({
        org,
        id: document.id,
        spaceId: space,
        folderIds: folders,
        sourceKey,
        existingId: existing?.id,
        revision: existing?.revision,
        roomId,
        reviewedContent: {
          title: title.trim(),
          markdown: exported.markdown,
          accessVersion: data.access_version,
        },
      });
      setResult(note);
      onPublished(note);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const verb = existing
    ? "Update published copy now"
    : `Publish to ${destination ? collectionName(destination) : "collection"} now`;
  return (
    <TeamDialog
      title={
        result
          ? existing
            ? "Shared copy updated"
            : "Document shared"
          : "Share a document with your team"
      }
      onClose={onClose}
      busy={busy}
    >
      {result ? (
        <div className="team-published">
          <Check size={22} />
          <h3>{result.title}</h3>
          <p>
            {existing ? "Updated in" : "Published to"} {data?.org.name} /{" "}
            {destination ? collectionName(destination) : ""}. Your team can find
            it through Search, and you can link it in a conversation.
          </p>
          <button className="team-primary" onClick={onClose}>
            Done
          </button>
        </div>
      ) : (
        <div className="team-form">
          <p>
            {existing ? (
              <>
                Replace the shared copy of <strong>{existing.title}</strong>{" "}
                (revision {existing.revision}) with this document as it is now.
              </>
            ) : (
              <>
                Share a copy of{" "}
                <strong>{document.title.trim() || "Untitled"}</strong>. Review
                the Markdown before sharing.
              </>
            )}
          </p>
          <label>
            Team
            <select
              aria-label="Team"
              value={org}
              onChange={(e) => setOrg(e.target.value)}
              disabled={busy || lockOrg}
            >
              <option value="">Choose a team</option>
              {orgs.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            Title
            <input
              value={title}
              required
              maxLength={500}
              disabled={busy}
              onChange={(e) => setTitle(e.target.value)}
            />
          </label>
          <label>
            Collection
            <select
              aria-label="Collection"
              value={space}
              onChange={(e) => {
                setSpace(e.target.value);
                setFolders([]);
              }}
              // A shared copy stays in the collection it was published to.
              disabled={busy || !!existing}
            >
              <option value="">Choose who gets access</option>
              {data?.spaces
                .filter(
                  (s) =>
                    (s.role === "editor" || s.id === existing?.space_id) &&
                    (!roomId || eligibleSpaces?.includes(s.id)),
                )
                .map((s) => (
                  <option key={s.id} value={s.id}>
                    {collectionName(s)} ·{" "}
                    {s.visibility === "team" ? "All members" : "Restricted"}
                  </option>
                ))}
            </select>
          </label>
          {roomId && eligibleSpaces?.length === 0 && (
            <p className="team-muted">
              No collection is currently writable and accessible to everyone in
              this conversation. Nothing has been published.
            </p>
          )}
          {destination && (
            <p className="team-audience">
              {destination.visibility === "restricted" ? (
                <Lock size={15} />
              ) : (
                <Users size={15} />
              )}
              {destination.visibility === "team"
                ? `Everyone in ${data?.org.name} can read this copy.`
                : "Team admins and members or groups with access to this collection can read this copy."}
            </p>
          )}
          {destination?.api_enabled && (
            <p className="team-muted">
              Approved team integrations can also read published content in this
              collection.
            </p>
          )}
          {!!data?.folders.filter((f) => f.space_id === space).length && (
            <fieldset>
              <legend>Folders (optional)</legend>
              {data?.folders
                .filter((f) => f.space_id === space)
                .map((f) => (
                  <label key={f.id} className="team-checkbox">
                    <input
                      type="checkbox"
                      checked={folders.includes(f.id)}
                      disabled={busy}
                      onChange={(e) =>
                        setFolders((old) =>
                          e.target.checked
                            ? [...old, f.id]
                            : old.filter((id) => id !== f.id),
                        )
                      }
                    />
                    {f.name}
                  </label>
                ))}
            </fieldset>
          )}
          <div className="team-publication-preview">
            {!exported ? (
              <p>Preparing the preview…</p>
            ) : empty ? (
              <p>This document is empty.</p>
            ) : (
              <MdBlock md={exported.markdown} />
            )}
          </div>
          {!!exported?.omitted.length && (
            <div className="team-omitted">
              <strong>Not included</strong>
              <ul>
                {exported.omitted.map((item, i) => (
                  <li key={i}>
                    {item.kind === "link"
                      ? item.detail
                      : item.kind === "image"
                        ? `Image: ${item.detail}`
                        : `${item.detail} block (kept as plain text)`}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {exported && (
            <p
              className={tooLong ? "team-error" : "team-muted"}
              role={tooLong ? "alert" : undefined}
            >
              {length.toLocaleString()} characters
              {tooLong &&
                ` — a shared copy holds at most ${MARKDOWN_LIMIT.toLocaleString()}. Split the document before sharing it.`}
            </p>
          )}
          {existing && !existing.can_edit && (
            <p className="team-error" role="alert">
              The shared copy is read-only for you now, so it cannot be updated
              from here.
            </p>
          )}
          <p className="team-muted">
            Images, alignment and text colour are not part of the shared copy.
            Publishing uploads a copy immediately. Removing its card or
            cancelling the chat message does not unpublish it. Later local edits
            stay on this Mac until you explicitly update the published copy.
          </p>
          {error && (
            <p className="team-error" role="alert">
              {error}
              <button
                type="button"
                className="team-text-button"
                disabled={busy}
                onClick={() => setReview((n) => n + 1)}
              >
                Refresh and review audience
              </button>
            </p>
          )}
          <button
            className="team-primary"
            onClick={() => void publish()}
            disabled={busy || !ready}
          >
            {busy
              ? existing
                ? "Updating…"
                : "Sharing…"
              : existing === undefined && org
                ? "Checking for a shared copy…"
                : verb}
          </button>
        </div>
      )}
    </TeamDialog>
  );
}
