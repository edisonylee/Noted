import { useEffect, useState } from "react";
import { Pin, MessageSquare } from "lucide-react";
import { TeamDialog } from "./TeamDialog";
import { orgPath, team } from "./client";
import type { TeamMessageLocation } from "./types";
import "./message-collections.css";
type Pinned = TeamMessageLocation & { pinned_at: string; pinned_by: string };
export function PinnedMessages({
  org,
  room,
  onClose,
  onOpen,
}: {
  org: string;
  room: string;
  onClose: () => void;
  onOpen: (id: string) => void;
}) {
  const [items, setItems] = useState<Pinned[]>([]),
    [error, setError] = useState(""),
    [loaded, setLoaded] = useState(false),
    [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const next = await team.request<Pinned[]>(
          "GET",
          orgPath(org, `/chat-rooms/${room}/pins`),
        );
        if (active) {
          setItems(next);
          setError("");
          setLoaded(true);
        }
      } catch (e) {
        if (active) {
          setItems([]);
          setError(String(e));
          setLoaded(true);
        }
      }
    };
    void load();
    const timer = setInterval(() => void load(), 10000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [org, room, retry]);
  return (
    <TeamDialog title="Pinned messages" onClose={onClose}>
      <p className="team-muted">Shared with everyone in this conversation.</p>
      {error && (
        <p className="team-error" role="alert">
          {error}
          <button
            className="team-text-button"
            onClick={() => setRetry((n) => n + 1)}
          >
            Retry
          </button>
        </p>
      )}
      {!loaded && <p role="status">Loading pinned messages…</p>}
      {loaded && !error && !items.length && (
        <div className="message-collection-empty">
          <Pin size={24} />
          <h3>No pinned messages</h3>
          <p>Pin a decision or useful link from a message’s actions.</p>
        </div>
      )}
      <div className="message-collection-list">
        {items.map((item) => (
          <button
            key={item.message.id}
            className="message-collection-item"
            onClick={() => {
              onClose();
              onOpen(item.message.id);
            }}
          >
            <strong>{item.message.author_name}</strong>
            <span>{item.message.body || "File attachment"}</span>
            <small>
              Pinned by {item.pinned_by} ·{" "}
              {new Date(item.pinned_at).toLocaleDateString()}
              {item.message.thread_id && (
                <>
                  {" "}
                  · <MessageSquare size={11} /> Thread reply
                </>
              )}
            </small>
          </button>
        ))}
      </div>
    </TeamDialog>
  );
}
