import { useEffect, useRef, useState } from "react";
import { Bookmark, RefreshCw, X } from "lucide-react";
import { team, orgPath } from "./client";
import { roomLabel } from "./messaging";
import type { TeamMessageLocation } from "./types";
import "./message-collections.css";
type Page = {
  items: (TeamMessageLocation & { saved_at: string })[];
  next_before: number | null;
};
export function SavedMessages({
  org,
  user,
  onOpen,
}: {
  org: string;
  user: string;
  onOpen: (id: string) => void;
}) {
  const [page, setPage] = useState<Page>({ items: [], next_before: null }),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(true),
    [removing, setRemoving] = useState(""),
    [retry, setRetry] = useState(0);
  const epoch = useRef(0);
  useEffect(() => {
    const version = ++epoch.current;
    setBusy(true);
    setPage({ items: [], next_before: null });
    team
      .request<Page>("GET", orgPath(org, "/saved-messages"))
      .then((next) => {
        if (version === epoch.current) {
          setPage(next);
          setError("");
        }
      })
      .catch((e) => {
        if (version === epoch.current) setError(String(e));
      })
      .finally(() => {
        if (version === epoch.current) setBusy(false);
      });
    const focus = () => setRetry((n) => n + 1);
    window.addEventListener("focus", focus);
    return () => {
      ++epoch.current;
      window.removeEventListener("focus", focus);
    };
  }, [org, user, retry]);
  return (
    <section className="mentions-inbox" aria-label="Saved messages">
      <header className="messages-room-head">
        <div>
          <h1>
            <Bookmark size={20} /> Saved messages
          </h1>
          <p>Private bookmarks for follow-up. Only you can see this list.</p>
        </div>
        <button
          className="team-text-button"
          aria-label="Refresh saved messages"
          onClick={() => setRetry((n) => n + 1)}
        >
          <RefreshCw size={16} />
        </button>
      </header>
      {error && (
        <p role="alert" className="team-error">
          {error}
        </p>
      )}
      <div className="mentions-list">
        {page.items.map((item) => (
          <div className="saved-message-row" key={item.message.id}>
            <button
              className="message-collection-item"
              onClick={() => onOpen(item.message.id)}
            >
              <strong>{item.message.author_name}</strong>
              <small>
                {roomLabel(item.room, user)}
                {item.parent ? " · Thread reply" : ""} · Saved{" "}
                {new Date(item.saved_at).toLocaleDateString()}
              </small>
              <span>
                {item.message.body ||
                  (item.message.has_meeting
                    ? "Shared meeting"
                    : "File attachment")}
              </span>
            </button>
            <button
              className="team-text-button"
              aria-label={`Unsave message from ${item.message.author_name}`}
              disabled={!!removing}
              onClick={async () => {
                const version = epoch.current;
                setRemoving(item.message.id);
                try {
                  await team.request(
                    "PUT",
                    orgPath(org, `/chat-messages/${item.message.id}/saved`),
                    { active: false },
                  );
                  if (version === epoch.current)
                    setPage((old) => ({
                      ...old,
                      items: old.items.filter(
                        (i) => i.message.id !== item.message.id,
                      ),
                    }));
                } catch (e) {
                  if (version === epoch.current) setError(String(e));
                } finally {
                  if (version === epoch.current) setRemoving("");
                }
              }}
            >
              <X size={14} />
            </button>
          </div>
        ))}
        {busy && (
          <p className="messages-empty" role="status">
            Loading saved messages…
          </p>
        )}
        {!busy && !error && !page.items.length && (
          <div className="message-collection-empty">
            <Bookmark size={26} />
            <h3>Keep useful messages close</h3>
            <p>Choose Save message privately from a message’s actions.</p>
          </div>
        )}
        {page.next_before != null && (
          <button
            className="team-text-button"
            disabled={busy}
            onClick={async () => {
              const version = epoch.current;
              setBusy(true);
              try {
                const next = await team.request<Page>(
                  "GET",
                  orgPath(org, `/saved-messages?before=${page.next_before}`),
                );
                if (version === epoch.current)
                  setPage((old) => ({
                    items: [
                      ...new Map(
                        [...old.items, ...next.items].map((i) => [
                          i.message.id,
                          i,
                        ]),
                      ).values(),
                    ],
                    next_before: next.next_before,
                  }));
              } catch (e) {
                if (version === epoch.current) setError(String(e));
              } finally {
                if (version === epoch.current) setBusy(false);
              }
            }}
          >
            Load more saved messages
          </button>
        )}
      </div>
    </section>
  );
}
