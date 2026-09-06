import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
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
  Plus,
  RefreshCw,
  Send,
  Settings2,
  Users,
  Search,
  SquarePen,
  X,
} from "lucide-react";
import { orgPath, team } from "./client";
import { TeamDialog } from "./TeamDialog";
import type {
  TeamChatMessage,
  TeamChatPage,
  TeamChatRoom,
  TeamSnapshot,
  TeamUser,
} from "./types";
import { mergeMessages, roomLabel } from "./messaging";
import { TeamAvatar } from "./TeamAvatar";
import { TeamProfileCard } from "./TeamProfile";
import { MessageRow } from "./MessageRow";
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
          <TeamAvatar
            org={org}
            person={
              data.members.find(
                (p) =>
                  p.id === room.participants.find((p) => p.id !== user)?.id,
              ) ??
              room.participants.find((p) => p.id !== user) ?? {
                id: "",
                name: roomLabel(room, user),
              }
            }
          />
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
          members={data.members}
          drafts={drafts}
          setDraftFor={(key, value) =>
            setDrafts((old) => ({ ...old, [key]: value }))
          }
          sendKeyFor={(key, body) => {
            if (attempts.current[key]?.body !== body)
              attempts.current[key] = { body, id: crypto.randomUUID() };
            return attempts.current[key].id;
          }}
          onSentFor={(key, body) => {
            delete attempts.current[key];
            setDrafts((old) =>
              old[key]?.trim() === body ? { ...old, [key]: "" } : old,
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
  members,
  drafts,
  setDraftFor,
  sendKeyFor,
  onSentFor,
  onRoom,
  onRead,
  thread,
  onCloseThread,
}: {
  org: string;
  user: string;
  room: TeamChatRoom;
  active: boolean;
  memberCount: number;
  members: TeamUser[];
  drafts: Record<string, string>;
  setDraftFor: (key: string, value: string) => void;
  sendKeyFor: (key: string, body: string) => string;
  onSentFor: (key: string, body: string) => void;
  onRoom: (room: TeamChatRoom) => void;
  onRead: (id: string) => void;
  thread?: TeamChatMessage;
  onCloseThread?: () => void;
}) {
  const draftKey = thread?.id ?? room.id;
  const draft = drafts[draftKey] ?? "";
  const setDraft = (value: string) => setDraftFor(draftKey, value);
  const [threadRoot, setThreadRoot] = useState<TeamChatMessage | null>(null);
  const [parent, setParent] = useState(thread);
  const [profile, setProfile] = useState<TeamUser | null>(null);
  const threadId = thread?.id;
  const canSend = room.can_send && !parent?.deleted_at;
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
  const prepend = useRef<{ height: number; top: number } | null>(null);
  const [scrollVersion, setScrollVersion] = useState(0);
  const visible = useRef(isActive);
  visible.current = isActive;
  const id = room.id;
  const path = orgPath(org, `/chat-rooms/${id}`);
  useEffect(() => {
    if (threadId && isActive) composer.current?.focus();
  }, [threadId, isActive]);
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
    setScrollVersion((value) => value + 1);
  }, []);
  useLayoutEffect(() => {
    const el = viewport.current;
    if (!el || !isActive) return;
    if (prepend.current) {
      el.scrollTop =
        prepend.current.top + el.scrollHeight - prepend.current.height;
      prepend.current = null;
    } else if (pinned.current) {
      el.scrollTop = el.scrollHeight;
      if (cursor.current != null) void acknowledge(cursor.current);
    }
  }, [messages, isActive, loaded, scrollVersion, acknowledge, parent]);
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
        const params = new URLSearchParams();
        if (threadId) params.set("thread", threadId);
        if (!initial) {
          params.set("after", String(after));
          params.set("wait", "20000");
        }
        const page = await team.request<TeamChatPage>(
          "GET",
          `${path}/messages?${params}`,
        );
        if (!active) return;
        if (epoch !== accessEpoch.current) {
          timer = window.setTimeout(poll, 1_000);
          return;
        }
        onRoom(page.room);
        if (threadId) setParent(page.parent);
        delay = page.live ? 0 : 3_000;
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
        if (
          !pinned.current &&
          page.messages.some(
            (m) => !m.deleted_at && (initial || m.created_seq > after!),
          )
        )
          setNewBelow(true);
        if (!initial && page.has_more) delay = 50;
      } catch (e) {
        if (!active) return;
        ++accessEpoch.current;
        setError(String(e));
        setMessages([]);
        setEditing(null);
        setDeleting(null);
        setSettings(false);
        setThreadRoot(null);
        setProfile(null);
        setParent(undefined);
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
  }, [path, onRoom, retry, isActive, threadId]);
  const older = async () => {
    if (olderBefore == null || loadingOlder) return;
    setLoadingOlder(true);
    const epoch = accessEpoch.current;
    const position = {
      height: viewport.current?.scrollHeight ?? 0,
      top: viewport.current?.scrollTop ?? 0,
    };
    try {
      const page = await team.request<TeamChatPage>(
        "GET",
        `${path}/messages?before=${olderBefore}${threadId ? `&thread=${threadId}` : ""}`,
      );
      if (!alive.current || epoch !== accessEpoch.current) return;
      prepend.current = position;
      setMessages((old) => mergeMessages(old, page.messages));
      setOlderBefore(page.older_before);
    } catch (e) {
      if (alive.current && epoch === accessEpoch.current) {
        ++accessEpoch.current;
        setError(String(e));
        setMessages([]);
        setEditing(null);
        setDeleting(null);
        setSettings(false);
        setThreadRoot(null);
        setParent(undefined);
        setProfile(null);
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
    if (!isActive || !body || sending || !canSend || error) return;
    const clientId = sendKeyFor(draftKey, body),
      epoch = accessEpoch.current;
    setSending(true);
    setSendError("");
    try {
      const message = await team.request<TeamChatMessage>(
        "POST",
        `${path}/messages`,
        { body, client_id: clientId, thread_id: threadId },
      );
      onSentFor(draftKey, body);
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
  const changed = (message: TeamChatMessage) => {
    if (!alive.current || renderEpoch !== accessEpoch.current) return;
    if (threadId && message.id === threadId) setParent(message);
    else setMessages((old) => mergeMessages(old, [message]));
  };
  const messageRow = (message: TeamChatMessage, showReplies = true) => {
    const person = members.find((p) => p.id === message.author_id) ?? {
      id: message.author_id,
      name: message.author_name,
    };
    return (
      <MessageRow
        key={message.id}
        org={org}
        user={user}
        message={message}
        person={person}
        canSend={canSend}
        showReplies={showReplies}
        extras={!!room.message_extras}
        onChanged={changed}
        onReply={() =>
          threadId ? composer.current?.focus() : setThreadRoot(message)
        }
        onEdit={() => setEditing(message)}
        onDelete={() => setDeleting(message)}
        onProfile={() => setProfile(person)}
      />
    );
  };
  return (
    <section
      className="messages-room"
      aria-label={threadId ? "Thread" : `Conversation: ${label}`}
      onKeyDown={(event) => {
        if (
          threadId &&
          event.key === "Escape" &&
          !document.querySelector("dialog[open]")
        ) {
          event.stopPropagation();
          onCloseThread?.();
        }
      }}
    >
      <div className="messages-room-main" inert={!!threadRoot || undefined}>
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
              {threadId ? "Thread" : label}
            </h1>
            <p>
              {threadId
                ? `Replies in ${label}`
                : room.archived_at
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
          {threadId && (
            <button
              className="team-text-button"
              aria-label="Close thread"
              onClick={onCloseThread}
            >
              <X size={18} />
            </button>
          )}
          {!threadId && room.can_manage && (
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
          {parent && (
            <div className="message-thread-parent">
              {messageRow(parent, false)}
              <div className="message-thread-divider">
                {parent.reply_count ?? 0}{" "}
                {(parent.reply_count ?? 0) === 1 ? "reply" : "replies"}
              </div>
            </div>
          )}
          {loaded && !error && messages.length === 0 && (
            <div className="messages-empty">
              <MessageSquare size={27} />
              <h2>
                {threadId
                  ? "Start the discussion"
                  : room.kind === "direct"
                    ? `Your conversation with ${label}`
                    : room.is_default
                      ? "A conversation for the whole team"
                      : `Welcome to #${label}`}
              </h2>
              <p>
                {threadId
                  ? "Replies stay with this message so the main conversation stays focused."
                  : room.kind === "direct"
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
                {messageRow(message, !threadId)}
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
            htmlFor={`compose-${draftKey}`}
          >
            {threadId
              ? "Reply in thread"
              : room.kind === "channel" && !room.is_default
                ? `Message #${label}`
                : `Message ${label}`}
          </label>
          <textarea
            id={`compose-${draftKey}`}
            ref={composer}
            value={draft}
            maxLength={10_000}
            rows={2}
            disabled={sending || !canSend}
            placeholder={
              canSend
                ? threadId
                  ? "Reply in thread"
                  : `Message ${room.kind === "channel" && !room.is_default ? "#" : ""}${label}`
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
              disabled={sending || !canSend || !draft.trim() || !!error}
            >
              {sending ? (
                <Loader size={14} className="spin" />
              ) : (
                <Send size={14} />
              )}{" "}
              {threadId ? "Reply" : "Send"}
            </button>
          </div>
        </form>
      </div>
      {threadRoot && !threadId && (
        <div className="messages-thread-panel">
          <MessageRoom
            key={threadRoot.id}
            org={org}
            user={user}
            room={room}
            active={isActive}
            memberCount={memberCount}
            members={members}
            thread={threadRoot}
            drafts={drafts}
            setDraftFor={setDraftFor}
            sendKeyFor={sendKeyFor}
            onSentFor={onSentFor}
            onRoom={onRoom}
            onRead={onRead}
            onCloseThread={() => {
              setThreadRoot(null);
              requestAnimationFrame(() => composer.current?.focus());
            }}
          />
        </div>
      )}
      {profile && (
        <TeamProfileCard
          org={org}
          person={profile}
          onClose={() => setProfile(null)}
        />
      )}
      {editing && (
        <EditMessage
          org={org}
          message={editing}
          onClose={() => setEditing(null)}
          onSaved={(message) => {
            if (!alive.current || renderEpoch !== accessEpoch.current) return;
            changed(message);
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
            changed(message);
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
