import { useEffect, useRef, useState } from "react";
import { Loader, MessagesSquare, RefreshCw, X } from "lucide-react";
import { orgPath, team } from "./client";
import { messagePreview, roomLabel, shortTime } from "./messaging";
import { mergeThreads, threadParticipants } from "./threads";
import type {
  TeamChatMessage,
  TeamChatRoom,
  TeamThreadPage,
  TeamThreadSummary,
} from "./types";
import "./message-collections.css";

// Reply traffic re-emits each root through the long poll, so a busy thread
// can request several refreshes per second; space page-one fetches out.
const REFRESH_SPACING = 2_000;

export function ThreadList({
  id,
  org,
  user,
  room,
  active,
  version,
  focusRoot,
  onOpen,
  onClose,
}: {
  id: string;
  org: string;
  user: string;
  room: TeamChatRoom;
  active: boolean;
  version: number;
  focusRoot?: string | null;
  onOpen: (root: TeamChatMessage) => void;
  onClose: () => void;
}) {
  const [items, setItems] = useState<TeamThreadSummary[]>([]),
    [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(""),
    [retry, setRetry] = useState(0);
  const [nextBefore, setNextBefore] = useState<number | null>(null),
    [loadingMore, setLoadingMore] = useState(false);
  const epoch = useRef(0),
    lastFetch = useRef(0);
  const visible = useRef(active);
  visible.current = active;
  const list = useRef<HTMLDivElement>(null),
    closeButton = useRef<HTMLButtonElement>(null),
    retryButton = useRef<HTMLButtonElement>(null);
  const path = orgPath(org, `/chat-rooms/${room.id}/threads`);
  useEffect(() => {
    const current = ++epoch.current;
    let timer: number;
    const load = async () => {
      if (!visible.current || document.visibilityState !== "visible") {
        timer = window.setTimeout(load, 10_000);
        return;
      }
      lastFetch.current = Date.now();
      try {
        const page = await team.request<TeamThreadPage>("GET", path);
        if (current !== epoch.current) return;
        // Page one is authoritative for its own range so a thread whose
        // replies were all deleted drops out; older loaded pages are kept.
        setItems((old) =>
          mergeThreads(
            old.filter(
              (thread) =>
                page.next_before != null &&
                thread.last_reply_seq <= page.next_before,
            ),
            page.items,
          ),
        );
        setNextBefore((old) => (old == null ? page.next_before : old));
        setError("");
      } catch (e) {
        if (current !== epoch.current) return;
        setItems([]);
        setNextBefore(null);
        setError(String(e));
      }
      setLoaded(true);
      timer = window.setTimeout(load, 10_000);
    };
    timer = window.setTimeout(
      load,
      Math.max(0, REFRESH_SPACING - (Date.now() - lastFetch.current)),
    );
    const wake = () => setRetry((n) => n + 1);
    window.addEventListener("focus", wake);
    return () => {
      ++epoch.current;
      clearTimeout(timer);
      window.removeEventListener("focus", wake);
    };
  }, [path, retry, version]);
  const loadMore = async () => {
    if (nextBefore == null || loadingMore) return;
    const current = epoch.current;
    setLoadingMore(true);
    try {
      const page = await team.request<TeamThreadPage>(
        "GET",
        `${path}?before=${nextBefore}`,
      );
      if (current !== epoch.current) return;
      setItems((old) => mergeThreads(old, page.items));
      setNextBefore(page.next_before);
    } catch (e) {
      if (current === epoch.current) setError(String(e));
    } finally {
      setLoadingMore(false);
    }
  };
  const rows = () =>
    Array.from(
      list.current?.querySelectorAll<HTMLElement>(".thread-list-row") ?? [],
    );
  // Focus lands once per arrival: the row the user came back from, else the
  // first row, else whatever control the current state offers. The main pane
  // is inert while the list is open, so focus must never fall to <body>
  // (Escape would stop reaching the section): a failure that unmounts the
  // rows re-arms the pass so it lands on Retry, and Retry re-arms it so a
  // recovery lands back on a row once the Retry button is gone.
  const focusPending = useRef(true),
    lastError = useRef("");
  useEffect(() => {
    focusPending.current = true;
  }, [focusRoot]);
  useEffect(() => {
    if (error && !lastError.current) focusPending.current = true;
    lastError.current = error;
    if (!loaded || !active || !focusPending.current) return;
    focusPending.current = false;
    const all = rows();
    const target = error
      ? retryButton.current
      : (all.find((row) => row.dataset.threadId === focusRoot) ??
        all[0] ??
        closeButton.current);
    target?.focus({ preventScroll: true });
  }, [loaded, active, error, focusRoot]);
  const count = items.length;
  return (
    <section
      className="messages-room messages-thread-list"
      id={id}
      aria-label="Threads"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          if (document.querySelector("dialog[open]")) return;
          event.stopPropagation();
          onClose();
          return;
        }
        const all = rows();
        if (!all.length) return;
        const index = all.indexOf(document.activeElement as HTMLElement);
        let next: number | undefined;
        if (event.key === "ArrowDown")
          next = index < 0 ? 0 : Math.min(all.length - 1, index + 1);
        else if (event.key === "ArrowUp")
          next = index < 0 ? all.length - 1 : Math.max(0, index - 1);
        else if (index >= 0 && event.key === "Home") next = 0;
        else if (index >= 0 && event.key === "End") next = all.length - 1;
        if (next === undefined) return;
        event.preventDefault();
        all[next].focus();
      }}
    >
      <header className="messages-room-head">
        <div>
          <h1>
            <MessagesSquare size={20} /> Threads
          </h1>
          <p>Discussions in {roomLabel(room, user)} · Newest reply first</p>
        </div>
        <button
          ref={closeButton}
          className="team-text-button"
          aria-label="Close threads"
          title="Close threads"
          onClick={onClose}
        >
          <X size={18} />
        </button>
      </header>
      <p className="sr-only" aria-live="polite">
        {loaded && !error
          ? `${count} ${count === 1 ? "thread" : "threads"}`
          : ""}
      </p>
      {!loaded && (
        <p className="messages-empty" role="status">
          <Loader size={16} className="spin" /> Loading threads…
        </p>
      )}
      {error && (
        <p className="team-error messages-thread-list-error" role="alert">
          {error}
          <button
            ref={retryButton}
            className="team-text-button"
            onClick={() => {
              focusPending.current = true;
              setRetry((n) => n + 1);
            }}
          >
            <RefreshCw size={14} /> Retry
          </button>
        </p>
      )}
      {loaded && !error && !count && (
        <div className="message-collection-empty">
          <MessagesSquare size={24} />
          <h3>No threads yet</h3>
          <p>Reply in thread from any message to start one.</p>
        </div>
      )}
      <div className="message-collection-list" ref={list}>
        {items.map((thread) => {
          const root = thread.root;
          const replies = thread.reply_count;
          const unread = thread.unread_replies;
          return (
            <button
              key={root.id}
              data-thread-id={root.id}
              className={`message-collection-item thread-list-row${unread ? " is-unread" : ""}`}
              aria-label={`Thread started by ${root.author_name}, ${replies} ${replies === 1 ? "reply" : "replies"}${unread ? `, ${unread} unread` : ""}`}
              onClick={() => onOpen(root)}
            >
              <span className="thread-list-head">
                <strong>{root.author_name}</strong>
                <time dateTime={root.created_at}>
                  {shortTime(root.created_at)}
                </time>
              </span>
              <span
                className={root.deleted_at ? "messages-deleted" : undefined}
              >
                {messagePreview(root)}
              </span>
              <small>
                {replies} {replies === 1 ? "reply" : "replies"} ·{" "}
                {threadParticipants(
                  thread.participants,
                  thread.participant_count,
                  user,
                )}{" "}
                · Last reply {shortTime(thread.last_reply_at)}
                {unread > 0 && (
                  <span
                    className="message-list-badge"
                    aria-label={`${unread} new replies`}
                  >
                    {unread}
                  </span>
                )}
              </small>
            </button>
          );
        })}
        {nextBefore != null && (
          <button
            className="team-text-button messages-older"
            disabled={loadingMore}
            onClick={() => void loadMore()}
          >
            {loadingMore ? "Loading…" : "Load older threads"}
          </button>
        )}
      </div>
    </section>
  );
}
