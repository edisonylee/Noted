import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import {
  ArrowDown,
  Hash,
  Lock,
  Loader,
  MessageSquare,
  Pencil,
  Plus,
  RefreshCw,
  Send,
  Settings2,
  Users,
  Search,
  SquarePen,
  Trash2,
} from "lucide-react";
import { orgPath, team } from "./client";
import { TeamDialog } from "./TeamDialog";
import type {
  TeamChatMessage,
  TeamChatPage,
  TeamChatRoom,
  TeamSnapshot,
} from "./types";
import { mergeMessages, roomLabel } from "./messaging";
import { initials } from "./presentation";
import "./messages.css";

export function TeamMessages({
  data,
  active,
  requestedRoom,
  onUnread,
}: {
  data: TeamSnapshot;
  active: boolean;
  requestedRoom: TeamChatRoom | null;
  onUnread: (count: number) => void;
}) {
  const org = data.org.id,
    user = data.user.id;
  const [rooms, setRooms] = useState<TeamChatRoom[]>([]);
  const [selected, setSelected] = useState("");
  const [search, setSearch] = useState("");
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  const [dialog, setDialog] = useState<"channel" | "direct" | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const attempts = useRef<Record<string, { body: string; id: string }>>({});
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true;
    let timer: number;
    const load = async () => {
      if (document.visibilityState !== "visible") {
        timer = window.setTimeout(load, 10_000);
        return;
      }
      try {
        const next = await team.request<TeamChatRoom[]>(
          "GET",
          orgPath(org, "/chat-rooms"),
        );
        if (!active) return;
        setRooms(next);
        setError("");
        setLoaded(true);
        setSelected((old) =>
          old && next.some((r) => r.id === old)
            ? old
            : (next.find((r) => r.is_default)?.id ?? next[0]?.id ?? ""),
        );
      } catch (e) {
        if (!active) return;
        setRooms([]);
        setDialog(null);
        setError(String(e));
        setLoaded(true);
      }
      if (active) timer = window.setTimeout(load, 10_000);
    };
    void load();
    const wake = () => {
      clearTimeout(timer);
      if (active) setRetry((v) => v + 1);
    };
    window.addEventListener("focus", wake);
    return () => {
      active = false;
      clearTimeout(timer);
      window.removeEventListener("focus", wake);
    };
  }, [org, retry]);
  const updateRoom = useCallback((room: TeamChatRoom) => {
    setRooms((old) =>
      old.some((r) => r.id === room.id)
        ? old.map((r) => (r.id === room.id ? room : r))
        : [...old, room],
    );
  }, []);
  const markRead = useCallback(
    (id: string) =>
      setRooms((old) =>
        old.map((r) => (r.id === id ? { ...r, unread: 0 } : r)),
      ),
    [],
  );
  useEffect(() => {
    onUnread(
      rooms
        .filter((room) => !room.archived_at)
        .reduce((total, room) => total + room.unread, 0),
    );
  }, [rooms, onUnread]);
  useEffect(() => {
    if (requestedRoom) {
      updateRoom(requestedRoom);
      setSelected(requestedRoom.id);
      setSearch("");
    }
  }, [requestedRoom, updateRoom]);
  useEffect(() => {
    if (!active) setDialog(null);
  }, [active]);
  const current = rooms.find((r) => r.id === selected);
  const matches = (room: TeamChatRoom) =>
    roomLabel(room, user).toLowerCase().includes(search.trim().toLowerCase());
  const directRooms = rooms
    .filter((room) => room.kind === "direct" && matches(room))
    .sort((a, b) => b.last_activity.localeCompare(a.last_activity));
  const channelRooms = rooms
    .filter(
      (room) =>
        room.kind === "channel" &&
        !room.is_default &&
        matches(room) &&
        (showArchived || !room.archived_at || room.id === selected),
    )
    .sort((a, b) => a.name.localeCompare(b.name));
  const roomButton = (room: TeamChatRoom) => (
    <button
      key={room.id}
      className={`messages-conversation-row${selected === room.id ? " on" : ""}`}
      onClick={() => setSelected(room.id)}
      aria-current={selected === room.id ? "page" : undefined}
    >
      <span
        className={`messages-person-mark${room.is_default ? " team" : ""}`}
        aria-hidden="true"
      >
        {room.is_default ? (
          <Users size={18} />
        ) : room.kind === "channel" ? (
          <Hash size={18} />
        ) : (
          initials(roomLabel(room, user))
        )}
      </span>
      <span className="messages-conversation-copy">
        <span className="messages-conversation-title">
          <strong>{roomLabel(room, user)}</strong>
          {room.last_message && (
            <time dateTime={room.last_message.created_at}>
              {new Date(room.last_message.created_at).toLocaleDateString() ===
              new Date().toLocaleDateString()
                ? new Date(room.last_message.created_at).toLocaleTimeString(
                    [],
                    { hour: "numeric", minute: "2-digit" },
                  )
                : new Date(room.last_message.created_at).toLocaleDateString(
                    [],
                    { month: "short", day: "numeric" },
                  )}
            </time>
          )}
        </span>
        <small>
          {drafts[room.id]?.trim()
            ? `Draft: ${drafts[room.id]}`
            : room.archived_at
              ? "Archived"
              : room.last_message
                ? `${room.last_message.author_id === user ? "You" : room.last_message.author_name}: ${room.last_message.body}`
                : room.is_default
                  ? `Everyone · ${data.members.length} members`
                  : room.kind === "direct"
                    ? "Private conversation"
                    : "Team channel"}
        </small>
      </span>
      {room.unread > 0 && (
        <b
          className="messages-unread"
          aria-label={`${room.unread} unread messages`}
        >
          {room.unread > 99 ? "99+" : room.unread}
        </b>
      )}
    </button>
  );
  return (
    <div className="team-messages">
      <aside
        className="team-sidebar messages-nav"
        aria-label="Team conversations"
      >
        <div className="messages-nav-head">
          <div className="messages-list-heading">
            <h2>Conversations</h2>
            <button
              aria-label="New message"
              title="New message"
              onClick={() => setDialog("direct")}
            >
              <SquarePen size={18} />
            </button>
          </div>
          <label className="messages-search">
            <Search size={14} />
            <input
              type="search"
              aria-label="Find a conversation"
              placeholder="Find a conversation"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </label>
        </div>
        {rooms
          .filter((room) => room.is_default && matches(room))
          .map(roomButton)}
        <div className="team-sidebar-label">
          <span>Direct messages</span>
          <button
            aria-label="New direct message"
            onClick={() => setDialog("direct")}
          >
            <Plus size={15} />
          </button>
        </div>
        {directRooms.map(roomButton)}
        {!directRooms.length && (
          <button
            className="messages-start-dm"
            onClick={() => setDialog("direct")}
          >
            <MessageSquare size={15} />
            {search ? "Find a teammate" : "Start a conversation"}
          </button>
        )}
        {(channelRooms.length > 0 || showArchived) && (
          <>
            <div className="team-sidebar-label">
              <span>Team channels</span>
              <button
                aria-label="Create channel"
                onClick={() => setDialog("channel")}
              >
                <Plus size={15} />
              </button>
            </div>
            {channelRooms.map(roomButton)}
          </>
        )}
        <div className="messages-nav-bottom">
          <button
            className="messages-create-topic"
            onClick={() => setDialog("channel")}
          >
            <Plus size={14} /> Create a team channel
          </button>
          <p>A shared conversation for a project or topic. Optional.</p>
          {rooms.some((room) => room.archived_at) && (
            <label className="messages-archived">
              <input
                type="checkbox"
                checked={showArchived}
                onChange={(event) => setShowArchived(event.target.checked)}
              />{" "}
              Show archived channels
            </label>
          )}
        </div>
      </aside>
      {current ? (
        <MessageRoom
          key={current.id}
          org={org}
          user={user}
          room={current}
          active={active}
          memberCount={data.members.length}
          draft={drafts[current.id] ?? ""}
          setDraft={(value) =>
            setDrafts((old) => ({ ...old, [current.id]: value }))
          }
          sendKey={(body) => {
            if (attempts.current[current.id]?.body !== body)
              attempts.current[current.id] = { body, id: crypto.randomUUID() };
            return attempts.current[current.id].id;
          }}
          onSent={(body) => {
            delete attempts.current[current.id];
            setDrafts((old) =>
              old[current.id]?.trim() === body
                ? { ...old, [current.id]: "" }
                : old,
            );
          }}
          onRoom={updateRoom}
          onRead={markRead}
        />
      ) : (
        <div className="messages-unavailable">
          {!loaded ? (
            <>
              <Loader size={18} className="spin" /> Loading conversations…
            </>
          ) : (
            <>
              <MessageSquare size={24} />
              <h2>{error ? "Chat is unavailable" : "Choose a conversation"}</h2>
              {error && (
                <p className="team-error" role="alert">
                  {error}
                </p>
              )}
              <button
                className="team-text-button"
                onClick={() => setRetry((v) => v + 1)}
              >
                <RefreshCw size={14} /> Retry
              </button>
            </>
          )}
        </div>
      )}
      {dialog && (
        <NewConversation
          key={dialog}
          onKindChange={setDialog}
          data={data}
          kind={dialog}
          onClose={() => setDialog(null)}
          onCreated={(room) => {
            updateRoom(room);
            setSelected(room.id);
            setDialog(null);
          }}
        />
      )}
    </div>
  );
}

function NewConversation({
  data,
  kind,
  onKindChange,
  onClose,
  onCreated,
}: {
  data: TeamSnapshot;
  kind: "channel" | "direct";
  onKindChange: (kind: "channel" | "direct") => void;
  onClose: () => void;
  onCreated: (room: TeamChatRoom) => void;
}) {
  const [name, setName] = useState(""),
    [description, setDescription] = useState(""),
    [member, setMember] = useState("");
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const people = data.members.filter(
    (m) =>
      m.id !== data.user.id &&
      m.name.toLowerCase().includes(name.toLowerCase()),
  );
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      onCreated(
        await team.request<TeamChatRoom>(
          "POST",
          orgPath(data.org.id, "/chat-rooms"),
          kind === "channel"
            ? { kind, name, description }
            : { kind, member_id: member },
        ),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <TeamDialog
      title={kind === "channel" ? "Create a team channel" : "New message"}
      busy={busy}
      onClose={onClose}
    >
      <form className="team-form" onSubmit={(e) => void submit(e)}>
        <div className="messages-kind-choice">
          <button
            type="button"
            aria-pressed={kind === "direct"}
            onClick={() => onKindChange("direct")}
          >
            Direct message
          </button>
          <button
            type="button"
            aria-pressed={kind === "channel"}
            onClick={() => onKindChange("channel")}
          >
            Team channel
          </button>
        </div>
        {kind === "channel" ? (
          <>
            <p className="team-muted">
              Everyone in {data.org.name} can read and send messages in this
              channel.
            </p>
            <label>
              Channel name
              <input
                value={name}
                maxLength={48}
                placeholder="project-launch"
                required
                onChange={(e) => setName(e.target.value)}
              />
            </label>
            <label>
              Description <span className="team-muted">optional</span>
              <input
                value={description}
                maxLength={500}
                onChange={(e) => setDescription(e.target.value)}
              />
            </label>
          </>
        ) : (
          <>
            <p className="team-muted">
              Only you and the selected teammate can open this conversation in
              Noted.
            </p>
            <label>
              Find a teammate
              <input
                type="search"
                value={name}
                placeholder="Search by name"
                onChange={(e) => setName(e.target.value)}
              />
            </label>
            <div className="messages-people">
              {people.map((person) => (
                <label key={person.id} className="messages-person-choice">
                  <input
                    type="radio"
                    name="teammate"
                    value={person.id}
                    checked={member === person.id}
                    onChange={() => setMember(person.id)}
                  />
                  <span>
                    {person.name}
                    <small>
                      {person.role === "member" ? "Member" : "Team admin"}
                    </small>
                  </span>
                </label>
              ))}
              {!people.length && (
                <p className="team-muted">
                  {data.members.length < 2
                    ? "Invite a teammate to start a direct message."
                    : "No teammates match that name."}
                </p>
              )}
            </div>
          </>
        )}
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button
          className="team-primary"
          disabled={busy || (kind === "direct" && !member)}
        >
          {busy && <Loader size={14} className="spin" />}
          {kind === "channel" ? "Create channel" : "Open conversation"}
        </button>
      </form>
    </TeamDialog>
  );
}

function MessageRoom({
  org,
  user,
  room,
  active: isActive,
  memberCount,
  draft,
  setDraft,
  sendKey,
  onSent,
  onRoom,
  onRead,
}: {
  org: string;
  user: string;
  room: TeamChatRoom;
  active: boolean;
  memberCount: number;
  draft: string;
  setDraft: (value: string) => void;
  sendKey: (body: string) => string;
  onSent: (body: string) => void;
  onRoom: (room: TeamChatRoom) => void;
  onRead: (id: string) => void;
}) {
  const [messages, setMessages] = useState<TeamChatMessage[]>([]),
    [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(""),
    [sendError, setSendError] = useState("");
  const [sending, setSending] = useState(false),
    [loadingOlder, setLoadingOlder] = useState(false);
  const [olderBefore, setOlderBefore] = useState<number | null>(null),
    [newBelow, setNewBelow] = useState(false);
  const [deleting, setDeleting] = useState<TeamChatMessage | null>(null);
  const [editing, setEditing] = useState<TeamChatMessage | null>(null),
    [settings, setSettings] = useState(false);
  const [retry, setRetry] = useState(0);
  const cursor = useRef<number | null>(null),
    readCursor = useRef(0);
  const viewport = useRef<HTMLDivElement>(null),
    composer = useRef<HTMLTextAreaElement>(null);
  const pinned = useRef(true),
    alive = useRef(true);
  const accessEpoch = useRef(0);
  const visible = useRef(isActive);
  visible.current = isActive;
  const id = room.id;
  const path = orgPath(org, `/chat-rooms/${id}`);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      ++accessEpoch.current;
    };
  }, []);
  const acknowledge = useCallback(
    async (seq: number) => {
      if (
        !visible.current ||
        seq <= readCursor.current ||
        document.visibilityState !== "visible"
      )
        return;
      try {
        await team.request("POST", `${path}/read`, { cursor: seq });
        if (alive.current) {
          readCursor.current = Math.max(seq, readCursor.current);
          onRead(id);
        }
      } catch {
        /* The next successful refresh retries the read marker. */
      }
    },
    [path, id, onRead],
  );
  const toBottom = useCallback(() => {
    pinned.current = true;
    setNewBelow(false);
    requestAnimationFrame(() => {
      if (!alive.current || !visible.current) return;
      viewport.current?.scrollTo({ top: viewport.current.scrollHeight });
      if (cursor.current != null) void acknowledge(cursor.current);
    });
  }, [acknowledge]);
  useEffect(() => {
    let active = true;
    let timer: number;
    const poll = async () => {
      if (!isActive || document.visibilityState !== "visible") {
        timer = window.setTimeout(poll, 3_000);
        return;
      }
      let delay = 3_000;
      const epoch = accessEpoch.current;
      try {
        const after = cursor.current;
        const initial = after == null;
        const page = await team.request<TeamChatPage>(
          "GET",
          `${path}/messages${initial ? "" : `?after=${after}`}`,
        );
        if (!active) return;
        if (epoch !== accessEpoch.current) {
          timer = window.setTimeout(poll, 1_000);
          return;
        }
        onRoom(page.room);
        setError("");
        setLoaded(true);
        setMessages((old) =>
          initial
            ? page.messages
            : mergeMessages(
                old,
                page.messages.filter(
                  (m) =>
                    m.created_seq > after! ||
                    old.some((existing) => existing.id === m.id),
                ),
              ),
        );
        if (initial) setOlderBefore(page.older_before);
        cursor.current = page.cursor;
        if (pinned.current) toBottom();
        else if (page.messages.some((m) => !m.deleted_at)) setNewBelow(true);
        if (!initial && page.has_more) delay = 50;
      } catch (e) {
        if (!active) return;
        ++accessEpoch.current;
        setError(String(e));
        setMessages([]);
        setEditing(null);
        setDeleting(null);
        setSettings(false);
        setLoaded(true);
        cursor.current = null;
        readCursor.current = 0;
        setOlderBefore(null);
        delay = 10_000;
      }
      if (active) timer = window.setTimeout(poll, delay);
    };
    void poll();
    const wake = () => {
      clearTimeout(timer);
      setRetry((v) => v + 1);
    };
    window.addEventListener("focus", wake);
    return () => {
      active = false;
      clearTimeout(timer);
      window.removeEventListener("focus", wake);
    };
  }, [path, onRoom, toBottom, retry, isActive]);
  const older = async () => {
    if (olderBefore == null || loadingOlder) return;
    setLoadingOlder(true);
    const epoch = accessEpoch.current;
    const beforeHeight = viewport.current?.scrollHeight ?? 0;
    try {
      const page = await team.request<TeamChatPage>(
        "GET",
        `${path}/messages?before=${olderBefore}`,
      );
      if (!alive.current || epoch !== accessEpoch.current) return;
      setMessages((old) => mergeMessages(old, page.messages));
      setOlderBefore(page.older_before);
      requestAnimationFrame(() => {
        if (viewport.current)
          viewport.current.scrollTop +=
            viewport.current.scrollHeight - beforeHeight;
      });
    } catch (e) {
      if (alive.current && epoch === accessEpoch.current) {
        ++accessEpoch.current;
        setError(String(e));
        setMessages([]);
        setEditing(null);
        setDeleting(null);
        setSettings(false);
        cursor.current = null;
        readCursor.current = 0;
        setOlderBefore(null);
      }
    } finally {
      if (alive.current) setLoadingOlder(false);
    }
  };
  const send = async () => {
    const body = draft.trim();
    if (!isActive || !body || sending || !room.can_send || error) return;
    const clientId = sendKey(body),
      epoch = accessEpoch.current;
    setSending(true);
    setSendError("");
    try {
      const message = await team.request<TeamChatMessage>(
        "POST",
        `${path}/messages`,
        { body, client_id: clientId },
      );
      onSent(body);
      if (!alive.current || epoch !== accessEpoch.current) return;
      setMessages((old) => mergeMessages(old, [message]));
      toBottom();
      requestAnimationFrame(() => composer.current?.focus());
    } catch (e) {
      if (alive.current) setSendError(String(e));
    } finally {
      if (alive.current) setSending(false);
    }
  };
  const label = roomLabel(room, user);
  const renderEpoch = accessEpoch.current;
  return (
    <section className="messages-room" aria-label={`Conversation: ${label}`}>
      <header className="messages-room-head">
        <div>
          <h1>
            {room.is_default ? (
              <Users size={21} />
            ) : room.kind === "channel" ? (
              <Hash size={21} />
            ) : (
              <Lock size={18} />
            )}
            {label}
          </h1>
          <p>
            {room.archived_at
              ? "Archived channel · History is available below"
              : room.is_default
                ? `Everyone in your team · ${memberCount} members`
                : room.kind === "direct"
                  ? room.can_send
                    ? "Private conversation · Only the two of you"
                    : "This teammate is no longer in the team"
                  : room.description || "Open to everyone in this team"}
          </p>
        </div>
        {room.can_manage && (
          <button
            className="team-text-button"
            aria-label={
              room.is_default ? "Team chat settings" : "Channel settings"
            }
            onClick={() => setSettings(true)}
          >
            <Settings2 size={17} />
          </button>
        )}
      </header>
      {error && (
        <div className="messages-connection-error" role="alert">
          {error}
          <button
            className="team-text-button"
            onClick={() => setRetry((v) => v + 1)}
          >
            <RefreshCw size={14} /> Retry
          </button>
        </div>
      )}
      <div
        className="messages-history"
        ref={viewport}
        role="log"
        aria-label="Message history"
        aria-live="polite"
        aria-relevant="additions text"
        onScroll={() => {
          const el = viewport.current;
          if (!el) return;
          pinned.current =
            el.scrollHeight - el.scrollTop - el.clientHeight < 80;
          if (pinned.current) {
            setNewBelow(false);
            if (cursor.current != null) void acknowledge(cursor.current);
          }
        }}
      >
        {olderBefore != null && (
          <button
            className="team-text-button messages-older"
            disabled={loadingOlder}
            onClick={() => void older()}
          >
            {loadingOlder ? "Loading…" : "Load earlier messages"}
          </button>
        )}
        {!loaded && (
          <p className="messages-empty">
            <Loader size={16} className="spin" /> Loading messages…
          </p>
        )}
        {loaded && !error && messages.length === 0 && (
          <div className="messages-empty">
            <MessageSquare size={27} />
            <h2>
              {room.kind === "direct"
                ? `Your conversation with ${label}`
                : room.is_default
                  ? "A conversation for the whole team"
                  : `Welcome to #${label}`}
            </h2>
            <p>
              {room.kind === "direct"
                ? "A place to follow up, ask a question, or share a thought."
                : "Keep the team in the loop. Everyone in this team can join the conversation."}
            </p>
          </div>
        )}
        {messages.map((message, index) => {
          const date = new Date(message.created_at);
          const day = date.toLocaleDateString(undefined, {
            month: "long",
            day: "numeric",
            year: "numeric",
          });
          const newDay =
            index === 0 ||
            new Date(messages[index - 1].created_at).toDateString() !==
              date.toDateString();
          return (
            <Fragment key={message.id}>
              {newDay && (
                <div className="messages-date">
                  <span>{day}</span>
                </div>
              )}
              <article
                className={`messages-message${message.author_id === user ? " own" : ""}`}
                aria-label={`Message from ${message.author_name}`}
              >
                <span className="messages-avatar" aria-hidden="true">
                  {message.author_name
                    .split(/\s+/)
                    .slice(0, 2)
                    .map((p) => p[0])
                    .join("")}
                </span>
                <div className="messages-message-content">
                  <header>
                    <strong>{message.author_name}</strong>
                    {message.author_id === user && <small>you</small>}
                    <time
                      dateTime={message.created_at}
                      title={date.toLocaleString()}
                    >
                      {date.toLocaleTimeString(undefined, {
                        hour: "numeric",
                        minute: "2-digit",
                      })}
                    </time>
                    {message.edited_at && !message.deleted_at && (
                      <small>edited</small>
                    )}
                  </header>
                  {message.deleted_at ? (
                    <p className="messages-deleted">Message deleted</p>
                  ) : (
                    <p>{message.body}</p>
                  )}
                </div>
                <div className="messages-actions">
                  {message.can_edit && room.can_send && (
                    <button
                      className="team-text-button"
                      aria-label={`Edit message from ${message.author_name}`}
                      onClick={() => setEditing(message)}
                    >
                      <Pencil size={13} />
                    </button>
                  )}
                  {message.can_delete && (
                    <button
                      className="team-text-button"
                      aria-label={`Delete message from ${message.author_name}`}
                      onClick={() => setDeleting(message)}
                    >
                      <Trash2 size={13} />
                    </button>
                  )}
                </div>
              </article>
            </Fragment>
          );
        })}
      </div>
      {newBelow && (
        <button className="messages-new team-primary" onClick={toBottom}>
          <ArrowDown size={14} /> New messages
        </button>
      )}
      <form
        className="messages-compose"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        {sendError && (
          <p className="team-error" role="alert">
            {sendError}
          </p>
        )}
        <label
          className="messages-compose-label sr-only"
          htmlFor={`compose-${id}`}
        >
          {room.kind === "channel" && !room.is_default
            ? `Message #${label}`
            : `Message ${label}`}
        </label>
        <textarea
          id={`compose-${id}`}
          ref={composer}
          value={draft}
          maxLength={10_000}
          rows={2}
          disabled={sending || !room.can_send}
          placeholder={
            room.can_send
              ? `Message ${room.kind === "channel" && !room.is_default ? "#" : ""}${label}`
              : "This conversation is read-only"
          }
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing
            ) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <div className="messages-compose-foot">
          <span>Enter to send · Shift + Enter for a new line</span>
          <button
            className="team-primary"
            disabled={sending || !room.can_send || !draft.trim() || !!error}
          >
            {sending ? (
              <Loader size={14} className="spin" />
            ) : (
              <Send size={14} />
            )}{" "}
            Send
          </button>
        </div>
      </form>
      {editing && (
        <EditMessage
          org={org}
          message={editing}
          onClose={() => setEditing(null)}
          onSaved={(message) => {
            if (!alive.current || renderEpoch !== accessEpoch.current) return;
            setMessages((old) => mergeMessages(old, [message]));
            setEditing(null);
          }}
        />
      )}
      {deleting && (
        <DeleteMessage
          org={org}
          message={deleting}
          onClose={() => setDeleting(null)}
          onDeleted={(message) => {
            if (!alive.current || renderEpoch !== accessEpoch.current) return;
            setMessages((old) => mergeMessages(old, [message]));
            setDeleting(null);
          }}
        />
      )}
      {settings && (
        <ChannelSettings
          org={org}
          room={room}
          onClose={() => setSettings(false)}
          onSaved={(room) => {
            if (!alive.current || renderEpoch !== accessEpoch.current) return;
            onRoom(room);
            setSettings(false);
          }}
        />
      )}
    </section>
  );
}

function DeleteMessage({
  org,
  message,
  onClose,
  onDeleted,
}: {
  org: string;
  message: TeamChatMessage;
  onClose: () => void;
  onDeleted: (message: TeamChatMessage) => void;
}) {
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  return (
    <TeamDialog title="Delete message?" busy={busy} onClose={onClose}>
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          setError("");
          try {
            onDeleted(
              await team.request<TeamChatMessage>(
                "DELETE",
                orgPath(org, `/chat-messages/${message.id}`),
                { revision: message.revision },
              ),
            );
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <p className="team-muted">
          This removes the message for everyone in this conversation. You cannot
          undo it.
        </p>
        <blockquote className="messages-delete-preview">
          {message.body}
        </blockquote>
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <div className="messages-delete-actions">
          <button
            type="button"
            className="team-text-button"
            disabled={busy}
            onClick={onClose}
          >
            Cancel
          </button>
          <button className="team-primary" disabled={busy}>
            {busy && <Loader size={14} className="spin" />} Delete message
          </button>
        </div>
      </form>
    </TeamDialog>
  );
}

function EditMessage({
  org,
  message,
  onClose,
  onSaved,
}: {
  org: string;
  message: TeamChatMessage;
  onClose: () => void;
  onSaved: (message: TeamChatMessage) => void;
}) {
  const [body, setBody] = useState(message.body),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  return (
    <TeamDialog title="Edit message" busy={busy} onClose={onClose}>
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          setError("");
          try {
            onSaved(
              await team.request<TeamChatMessage>(
                "PATCH",
                orgPath(org, `/chat-messages/${message.id}`),
                { body, revision: message.revision },
              ),
            );
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Message
          <textarea
            rows={5}
            value={body}
            maxLength={10_000}
            required
            onChange={(e) => setBody(e.target.value)}
          />
        </label>
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy || !body.trim()}>
          Save changes
        </button>
      </form>
    </TeamDialog>
  );
}

function ChannelSettings({
  org,
  room,
  onClose,
  onSaved,
}: {
  org: string;
  room: TeamChatRoom;
  onClose: () => void;
  onSaved: (room: TeamChatRoom) => void;
}) {
  const [name, setName] = useState(room.name),
    [description, setDescription] = useState(room.description);
  const [archived, setArchived] = useState(!!room.archived_at),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  return (
    <TeamDialog
      title={room.is_default ? "Team chat settings" : "Channel settings"}
      busy={busy}
      onClose={onClose}
    >
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          setError("");
          try {
            onSaved(
              await team.request<TeamChatRoom>(
                "PATCH",
                orgPath(org, `/chat-rooms/${room.id}`),
                { name, description, archived, revision: room.revision },
              ),
            );
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {!room.is_default && (
          <label>
            Channel name
            <input
              value={name}
              maxLength={48}
              required
              onChange={(e) => setName(e.target.value)}
            />
          </label>
        )}
        <label>
          Description
          <textarea
            rows={3}
            value={description}
            maxLength={500}
            onChange={(e) => setDescription(e.target.value)}
          />
        </label>
        {!room.is_default && (
          <label className="team-checkbox">
            <input
              type="checkbox"
              checked={archived}
              onChange={(e) => setArchived(e.target.checked)}
            />{" "}
            Archive channel. History remains available; new messages are paused.
          </label>
        )}
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy}>
          Save changes
        </button>
      </form>
    </TeamDialog>
  );
}
