import { useEffect, useRef, useState } from "react";
import { Share2 } from "lucide-react";
import { team, orgPath } from "./client";
import { TeamDialog } from "./TeamDialog";
import type { TeamNote, TeamChatMessage } from "./types";

type Target = { id: string; label: string; audience: string };
export function ShareMeetingDialog({
  org,
  note,
  onClose,
  onShared,
  initialQuote = "",
}: {
  org: string;
  note: TeamNote;
  onClose: () => void;
  onShared: (id: string) => void;
  initialQuote?: string;
}) {
  const [targets, setTargets] = useState<Target[]>([]),
    [room, setRoom] = useState(""),
    [quote, setQuote] = useState(initialQuote),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [loaded, setLoaded] = useState(false),
    [retry, setRetry] = useState(0);
  const attempt = useRef({ key: "", id: "" });
  useEffect(() => {
    let active = true;
    setLoaded(false);
    team
      .request<Target[]>("GET", orgPath(org, `/notes/${note.id}/share-targets`))
      .then((rows) => {
        if (active) {
          setTargets(rows);
          setError("");
          setLoaded(true);
        }
      })
      .catch((e) => {
        if (active) {
          setTargets([]);
          setError(String(e));
          setLoaded(true);
        }
      });
    return () => {
      active = false;
    };
  }, [org, note.id, retry]);
  const target = targets.find((t) => t.id === room),
    excerpt = quote.trim(),
    start = excerpt ? note.summary.indexOf(excerpt) : 0;
  return (
    <TeamDialog
      title="Share meeting in a conversation"
      busy={busy}
      onClose={onClose}
    >
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!target || start < 0 || busy) return;
          setBusy(true);
          setError("");
          try {
            const meeting = {
              id: note.id,
              revision: note.revision,
              start,
              length: excerpt.length,
            };
            const key = JSON.stringify([room, meeting]);
            if (attempt.current.key !== key)
              attempt.current = { key, id: crypto.randomUUID() };
            const message = await team.request<TeamChatMessage>(
              "POST",
              orgPath(org, `/chat-rooms/${room}/messages`),
              { body: "", client_id: attempt.current.id, meeting },
            );
            onShared(message.id);
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <div className="message-source-review">
          <strong>{note.title}</strong>
          <p className="team-muted">
            A link to the published meeting. Local notes stay private.
          </p>
        </div>
        <label>
          Conversation
          <select
            value={room}
            disabled={busy || !loaded}
            onChange={(e) => setRoom(e.target.value)}
            required
          >
            <option value="">Choose a conversation</option>
            {targets.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </select>
        </label>
        {!loaded && <p role="status">Checking available conversations…</p>}
        {loaded && !targets.length && !error && (
          <p className="team-muted">
            No eligible conversations. Everyone in the destination needs access
            to this meeting. Restricted meetings can be shared in eligible DMs.
          </p>
        )}
        <label>
          Quote from the meeting notes{" "}
          <small>Optional · up to 1,000 characters</small>
          <textarea
            rows={3}
            value={quote}
            maxLength={1000}
            disabled={busy}
            placeholder="Paste an exact decision or passage from the shared notes"
            onChange={(e) => setQuote(e.target.value)}
          />
        </label>
        {start < 0 && (
          <p className="team-error" role="alert">
            Choose an exact passage from the current shared meeting notes.
          </p>
        )}
        {target && (
          <p className="message-share-audience">
            <strong>Visible to:</strong> {target.audience}. Source access is
            checked again when sharing and opening.
          </p>
        )}
        {error && (
          <p className="team-error" role="alert">
            {error}
            <button
              type="button"
              className="team-text-button"
              disabled={busy}
              onClick={() => setRetry((n) => n + 1)}
            >
              Refresh destinations
            </button>
          </p>
        )}
        <button
          className="team-primary"
          disabled={busy || !target || start < 0}
        >
          <Share2 size={14} />
          {busy ? "Sharing…" : "Share meeting"}
        </button>
      </form>
    </TeamDialog>
  );
}
