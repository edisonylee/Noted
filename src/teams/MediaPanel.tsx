import { useEffect, useRef, useState } from "react";
import { FileText, Images, Loader, RefreshCw, X } from "lucide-react";
import { orgPath, team } from "./client";
import { roomLabel, shortTime } from "./messaging";
import {
  mediaCountLabel,
  mediaKey,
  mediaKinds,
  mergeMedia,
  type MediaKind,
} from "./media";
import { sizeLabel } from "./attachmentPreviewData";
import { useAttachmentPreview } from "./useAttachmentPreview";
import type {
  TeamAttachment,
  TeamChatRoom,
  TeamMediaItem,
  TeamMediaPage,
} from "./types";
import "./message-collections.css";

// Window focus and room traffic both ask for page one; space those fetches
// out so focus flapping cannot refetch back-to-back (as ThreadList does).
const REFRESH_SPACING = 2_000;

// What this conversation has shared, in the thread-panel slot with the thread
// list's conventions: one panel at a time, Escape closes, the main pane is
// inert while it is open, and the parent's resetThreadPanel() is the only exit.
export function MediaPanel({
  id,
  org,
  user,
  room,
  active,
  version,
  onJump,
  onOpenDocument,
  onClose,
}: {
  id: string;
  org: string;
  user: string;
  room: TeamChatRoom;
  active: boolean;
  version: number;
  onJump: (messageId: string) => void;
  onOpenDocument?: (noteId: string) => void;
  onClose: () => void;
}) {
  const [kind, setKind] = useState<MediaKind>("images");
  const [items, setItems] = useState<TeamMediaItem[]>([]),
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
  const path = orgPath(org, `/chat-rooms/${room.id}/media?kind=${kind}`);
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
        const page = await team.request<TeamMediaPage>("GET", path);
        if (current !== epoch.current) return;
        setItems((old) => mergeMedia(old, page, true));
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
      const page = await team.request<TeamMediaPage>(
        "GET",
        `${path}&before=${nextBefore}`,
      );
      if (current !== epoch.current) return;
      setItems((old) => mergeMedia(old, page, false));
      setNextBefore(page.next_before);
    } catch (e) {
      if (current === epoch.current) setError(String(e));
    } finally {
      setLoadingMore(false);
    }
  };
  const switchKind = (next: MediaKind) => {
    if (next === kind) return;
    // A chosen chip is a fresh list, not a refresh: it must not wait out the
    // spacing left by the previous kind's fetch.
    lastFetch.current = 0;
    setKind(next);
    setItems([]);
    setNextBefore(null);
    setLoaded(false);
    setError("");
  };
  const rows = () =>
    Array.from(
      list.current?.querySelectorAll<HTMLElement>(".media-panel-row") ?? [],
    );
  // Focus lands once per arrival, never on <body>: the main pane is inert, so
  // a focus that fell through would stop Escape from reaching the section.
  const focusPending = useRef(true),
    lastError = useRef("");
  useEffect(() => {
    if (error && !lastError.current) focusPending.current = true;
    lastError.current = error;
    if (!loaded || !active || !focusPending.current) return;
    focusPending.current = false;
    const target = error
      ? retryButton.current
      : (rows()[0] ?? closeButton.current);
    target?.focus({ preventScroll: true });
  }, [loaded, active, error]);
  const count = items.length;
  const empty = mediaKinds.find((k) => k.id === kind)!;
  return (
    <section
      className="messages-room messages-thread-list messages-media-panel"
      id={id}
      aria-label="Media"
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
            <Images size={20} /> Media
          </h1>
          <p>Shared in {roomLabel(room, user)} · Newest first</p>
        </div>
        <button
          ref={closeButton}
          className="team-text-button"
          aria-label="Close media"
          title="Close media"
          onClick={onClose}
        >
          <X size={18} />
        </button>
      </header>
      <div
        className="media-panel-filters"
        role="tablist"
        aria-label="Media kind"
        onKeyDown={(event) => {
          const order = mediaKinds.map((k) => k.id);
          const at = order.indexOf(kind);
          const next =
            event.key === "ArrowRight"
              ? order[(at + 1) % order.length]
              : event.key === "ArrowLeft"
                ? order[(at + order.length - 1) % order.length]
                : event.key === "Home"
                  ? order[0]
                  : event.key === "End"
                    ? order[order.length - 1]
                    : null;
          if (!next) return;
          event.preventDefault();
          event.stopPropagation();
          const group = event.currentTarget;
          switchKind(next);
          requestAnimationFrame(() =>
            group
              .querySelector<HTMLButtonElement>('[aria-selected="true"]')
              ?.focus(),
          );
        }}
      >
        {mediaKinds.map((option) => (
          <button
            key={option.id}
            role="tab"
            id={`${id}-tab-${option.id}`}
            aria-selected={option.id === kind}
            aria-controls={`${id}-list`}
            tabIndex={option.id === kind ? 0 : -1}
            onClick={() => switchKind(option.id)}
          >
            {option.label}
          </button>
        ))}
      </div>
      <p className="sr-only" aria-live="polite">
        {loaded && !error ? mediaCountLabel(kind, count) : ""}
      </p>
      {/* Every state lives in the tabpanel the chips control; the grid only
          lays out thumbnails, so a status or empty message keeps the block
          layout. */}
      <div
        className={`message-collection-list${kind === "images" && count ? " media-panel-grid" : ""}`}
        id={`${id}-list`}
        role="tabpanel"
        aria-labelledby={`${id}-tab-${kind}`}
        ref={list}
      >
        {!loaded && (
          <p className="messages-empty" role="status">
            <Loader size={16} className="spin" /> Loading {kind}…
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
            <Images size={24} />
            <h3>No {kind} shared yet</h3>
            <p>
              {kind === "documents"
                ? "Documents shared with /document appear here."
                : `${empty.label} sent in this conversation appear here.`}
            </p>
          </div>
        )}
        {items.map((item) =>
          item.attachment ? (
            kind === "images" ? (
              <MediaThumbnail
                key={mediaKey(item)}
                org={org}
                item={item}
                file={item.attachment}
                onOpen={() => onJump(item.message_id)}
              />
            ) : (
              <button
                key={mediaKey(item)}
                className="message-collection-item media-panel-row"
                onClick={() => onJump(item.message_id)}
              >
                <span className="media-panel-head">
                  <FileText size={16} aria-hidden="true" />
                  <strong title={item.attachment.name}>
                    {item.attachment.name}
                  </strong>
                </span>
                <small>
                  {sizeLabel(item.attachment.size)} · {item.author.name} ·{" "}
                  <time dateTime={item.created_at}>
                    {shortTime(item.created_at)}
                  </time>
                </small>
              </button>
            )
          ) : item.document ? (
            <button
              key={mediaKey(item)}
              className="message-collection-item media-panel-row"
              disabled={!onOpenDocument}
              onClick={() => onOpenDocument?.(item.document!.note_id)}
            >
              <span className="media-panel-head">
                <FileText size={16} aria-hidden="true" />
                <strong>{item.document.title}</strong>
              </span>
              <small>
                {item.author.name} ·{" "}
                <time dateTime={item.created_at}>
                  {shortTime(item.created_at)}
                </time>
                {item.document.updated && " · Updated since shared"}
              </small>
            </button>
          ) : null,
        )}
        {nextBefore != null && (
          <button
            className="team-text-button messages-older"
            disabled={loadingMore}
            onClick={() => void loadMore()}
          >
            {loadingMore ? "Loading…" : `Load older ${kind}`}
          </button>
        )}
      </div>
    </section>
  );
}

// The same fetch-and-blob path as the inline card, so nothing new is cached
// or exposed; loading waits until the tile scrolls into view.
function MediaThumbnail({
  org,
  item,
  file,
  onOpen,
}: {
  org: string;
  item: TeamMediaItem;
  file: TeamAttachment;
  onOpen: () => void;
}) {
  const tile = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  const [broken, setBroken] = useState(false);
  const { preview, error } = useAttachmentPreview(org, file, visible);
  useEffect(() => {
    const observer = new IntersectionObserver((entries) =>
      setVisible(entries[0].isIntersecting),
    );
    if (tile.current) observer.observe(tile.current);
    return () => observer.disconnect();
  }, []);
  return (
    <button
      ref={tile}
      className="media-panel-row media-panel-thumb"
      aria-label={`${file.name}, ${item.author.name}, ${shortTime(item.created_at)}`}
      title={`${file.name} · ${item.author.name} · ${shortTime(item.created_at)}`}
      onClick={onOpen}
    >
      {preview && !broken ? (
        <img
          src={preview.url}
          alt=""
          decoding="async"
          onError={() => setBroken(true)}
        />
      ) : (
        <span>
          {error || broken ? (
            <Images size={18} aria-hidden="true" />
          ) : (
            <Loader size={16} className="spin" aria-hidden="true" />
          )}
        </span>
      )}
    </button>
  );
}
