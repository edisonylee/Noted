import { useEffect, useState } from "react";
import { BookOpen, FileText, Search, X } from "lucide-react";
import { TeamDialog } from "./TeamDialog";
import { orgPath, team } from "./client";
import type { TeamNoteKind } from "./types";

// One staged source reference per message, meeting or document alike: the
// send payload, retry key and clearing never look at kind, only the card does.
export type PendingMeeting = {
  id: string;
  revision: number;
  title: string;
  occurred_at: string;
  collection: string;
  kind?: TeamNoteKind;
};
type Page = { meetings: PendingMeeting[]; next_offset: number | null };
const date = (value: string) =>
  new Date(value).toLocaleDateString([], {
    month: "short",
    day: "numeric",
    year: "numeric",
  });

export function StagedMeeting({
  meeting,
  disabled,
  onRemove,
}: {
  meeting: PendingMeeting;
  disabled: boolean;
  onRemove: () => void;
}) {
  const document = meeting.kind === "document";
  const Icon = document ? FileText : BookOpen;
  const noun = document ? "Document" : "Meeting";
  return (
    <div
      className="composer-staged-meeting"
      aria-label={`${noun} ready to send`}
    >
      <Icon size={18} aria-hidden="true" />
      <span>
        <strong>{meeting.title}</strong>
        <small>
          {noun} · {date(meeting.occurred_at)}
          {meeting.collection && ` · ${meeting.collection}`} · Ready to send
        </small>
      </span>
      <button
        type="button"
        className="icon-btn"
        disabled={disabled}
        onClick={onRemove}
        aria-label={`Remove ${noun.toLowerCase()} reference`}
      >
        <X size={15} />
      </button>
    </div>
  );
}

export function MeetingPicker({
  org,
  room,
  onClose,
  onChoose,
}: {
  org: string;
  room: string;
  onClose: () => void;
  onChoose: (meeting: PendingMeeting) => void;
}) {
  const [query, setQuery] = useState("");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<Page>({ meetings: [], next_offset: null });
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true;
    setBusy(true);
    setError("");
    const timer = window.setTimeout(
      () => {
        const params = new URLSearchParams({
          q: query,
          offset: String(offset),
        });
        team
          .request<Page>(
            "GET",
            orgPath(org, `/chat-rooms/${room}/meeting-targets?${params}`),
          )
          .then((result) => {
            if (active)
              setPage((old) => ({
                ...result,
                meetings: offset
                  ? [
                      ...old.meetings,
                      ...result.meetings.filter(
                        (item) =>
                          !old.meetings.some(
                            (existing) => existing.id === item.id,
                          ),
                      ),
                    ]
                  : result.meetings,
              }));
          })
          .catch((e) => {
            if (active) setError(String(e));
          })
          .finally(() => {
            if (active) setBusy(false);
          });
      },
      query ? 180 : 0,
    );
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [org, room, query, offset, retry]);
  return (
    <TeamDialog
      title="Reference a meeting"
      onClose={onClose}
      className="meeting-picker"
    >
      <p className="team-muted">
        Choose shared notes everyone in this conversation can access. Your
        message stays a draft until you send it.
      </p>
      <label className="meeting-picker-search">
        <Search size={16} aria-hidden="true" />
        <input
          aria-label="Search shared meetings"
          placeholder="Search meetings or collections"
          maxLength={200}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setOffset(0);
            setPage({ meetings: [], next_offset: null });
            setBusy(true);
          }}
        />
      </label>
      <div className="meeting-picker-results" aria-busy={busy}>
        {page.meetings.map((meeting) => (
          <button
            type="button"
            key={meeting.id}
            disabled={busy || !!error}
            onClick={() => onChoose(meeting)}
          >
            <BookOpen size={18} aria-hidden="true" />
            <span>
              <strong>{meeting.title}</strong>
              <small>
                {date(meeting.occurred_at)} · {meeting.collection}
              </small>
            </span>
          </button>
        ))}
        {busy && <p role="status">Finding meetings…</p>}
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
        {!busy && !error && !page.meetings.length && (
          <p className="team-muted">
            {query
              ? "No matching meetings."
              : "No shared meetings are available to everyone here yet."}
          </p>
        )}
        {page.next_offset !== null && !error && (
          <button
            type="button"
            className="team-text-button"
            disabled={busy}
            onClick={() => setOffset(page.next_offset!)}
          >
            Load more meetings
          </button>
        )}
      </div>
    </TeamDialog>
  );
}
