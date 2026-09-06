import { MessageComposer, type MessageComposerHandle } from "./MessageComposer";
import { messageFormatting } from "./messageFormatting";
import { SavedMessages } from "./SavedMessages";
import { PinnedMessages } from "./PinnedMessages";
import {
  AttachmentPicker,
  type AttachmentPickerHandle,
  type PendingAttachment,
} from "./MessageAttachments";
import { MessageSidebar } from "./MessageSidebar";
import { ConversationNotifications } from "./ConversationNotifications";
import { MentionsInbox } from "./MentionsInbox";
import type { TeamMessageLocation, TeamNotificationTarget } from "./types";
import {
  captureMessagePosition,
  readMessagePosition,
  restoreMessagePosition,
  saveMessagePosition,
} from "./messageScroll";
import { useNavigationState } from "../useNavigationState";
import { setViewedMessageRoom } from "./mentionNotifications";
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
  ChevronLeft,
  CornerUpLeft,
  Hash,
  Lock,
  Loader,
  MessageSquare,
  MessagesSquare,
  Paperclip,
  RefreshCw,
  Send,
  Settings2,
  Users,
  Search,
  X,
} from "lucide-react";
import { orgPath, team } from "./client";
import { TeamDialog } from "./TeamDialog";
import type {
  TeamChatMessage,
  TeamChatPage,
  TeamChatRoom,
  TeamReplyReference,
  TeamSnapshot,
  TeamUser,
} from "./types";
import {
  canReplyInline,
  mergeMessages,
  quotePreview,
  replyReference,
  roomLabel,
  sendAttemptKey,
} from "./messaging";
import {
  draftKey as draftStorageKey,
  readDrafts,
  writeDrafts,
} from "./messageDrafts";
import { findMentions, mentionQuery } from "./mentions";
import { TeamProfileCard } from "./TeamProfile";
import { MessageRow } from "./MessageRow";
import { ThreadList } from "./ThreadList";
import "./messages.css";

export function TeamMessages({
  data,
  active,
  requestedRoom,
  onUnread,
  onMeeting,
  notificationTarget,
  onNotificationHandled,
  requestedMessage,
  onSearch,
  onRequestedMessageHandled,
}: {
  requestedMessage?: { id: string; nonce: number } | null;
  onRequestedMessageHandled?: () => void;
  onSearch?: (room: string) => void;
  notificationTarget?: TeamNotificationTarget | null;
  onNotificationHandled?: () => void;
  data: TeamSnapshot;
  active: boolean;
  requestedRoom: TeamChatRoom | null;
  onUnread: (count: number) => void;
  onMeeting?: (id: string) => void;
}) {
  const org = data.org.id,
    user = data.user.id;
  const [rooms, setRooms] = useState<TeamChatRoom[]>([]);
  const [savedView, setSavedView] = useNavigationState(
    `team:${org}:${user}:saved-messages-view`,
    false,
  );
  const [inbox, setInbox] = useNavigationState(
    `team:${org}:${user}:mentions-inbox`,
    false,
  );
  const [jump, setJump] = useState<
    (TeamMessageLocation & { nonce: number }) | null
  >(null);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState("");
  const openEpoch = useRef(0);
  useEffect(
    () => () => {
      ++openEpoch.current;
    },
    [],
  );
  const openMessage = useCallback(
    async (message: string) => {
      const epoch = ++openEpoch.current;
      setOpening(true);
      setOpenError("");
      try {
        const location = await team.request<TeamMessageLocation>(
          "GET",
          orgPath(org, `/chat-messages/${encodeURIComponent(message)}`),
        );
        if (epoch !== openEpoch.current) return;
        setRooms((old) => [
          ...old.filter((r) => r.id !== location.room.id),
          location.room,
        ]);
        setSelected(location.room.id);
        setJump({ ...location, nonce: epoch });
        setInbox(false);
        setSavedView(false);
        setListVisible(false);
        // Opening one mention does not mark the entire conversation as read.
        await team.request(
          "POST",
          orgPath(org, `/mentions/${encodeURIComponent(message)}/read`),
        );
        if (epoch === openEpoch.current) setRetry((n) => n + 1);
      } catch (e) {
        if (epoch === openEpoch.current) setOpenError(String(e));
      } finally {
        if (epoch === openEpoch.current) setOpening(false);
      }
    },
    [org],
  );
  useEffect(() => {
    if (!notificationTarget) return;
    if (notificationTarget.org !== org || notificationTarget.user !== user) {
      setOpenError("This notification belongs to a different team account.");
      onNotificationHandled?.();
      return;
    }
    void openMessage(notificationTarget.message);
    onNotificationHandled?.();
  }, [notificationTarget, org, user, openMessage, onNotificationHandled]);
  useEffect(() => {
    if (requestedMessage) {
      void openMessage(requestedMessage.id);
      onRequestedMessageHandled?.();
    }
  }, [requestedMessage, openMessage, onRequestedMessageHandled]);
  const [selected, setSelected] = useNavigationState(
    `team:${org}:${user}:room`,
    "",
  );
  const [search, setSearch] = useNavigationState(
    `team:${org}:${user}:room-search`,
    "",
  );
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [listVisible, setListVisible] = useNavigationState(
    `team:${org}:${user}:message-list`,
    true,
  );
  const shell = useRef<HTMLDivElement>(null);
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    const el = shell.current;
    if (!compact || !el?.contains(document.activeElement)) return;
    const target = listVisible
      ? '.message-sidebar [aria-current="page"], .message-sidebar input'
      : ".messages-compact-toolbar button";
    el.querySelector<HTMLElement>(target)?.focus({ preventScroll: true });
  }, [compact, listVisible]);
  useLayoutEffect(() => {
    const el = shell.current;
    if (!el) return;
    const measure = () => {
      if (el.clientWidth) setCompact(el.clientWidth < 700);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const [dialog, setDialog] = useState<"channel" | "direct" | null>(null);
  // Master keys drafts by thread-or-room; persist that same map so a
  // half-written message survives a reload, not just a room switch.
  const storageKey = draftStorageKey(org, user);
  const [drafts, setDraftState] = useState(() => readDrafts(storageKey));
  const draftsRef = useRef(drafts);
  const [draftError, setDraftError] = useState(false);
  const setDrafts = (
    update: (old: Record<string, string>) => Record<string, string>,
  ) => {
    const next = update(draftsRef.current);
    draftsRef.current = next;
    setDraftState(next);
    setDraftError(!writeDrafts(storageKey, next));
  };
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
        old.map((r) =>
          r.id === id ? { ...r, unread: 0, unread_mentions: 0 } : r,
        ),
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
      setInbox(false);
      setSavedView(false);
      setJump(null);
      setListVisible(false);
      setSearch("");
    }
  }, [requestedRoom, updateRoom]);
  useEffect(() => {
    if (!active) setDialog(null);
  }, [active]);
  const current = rooms.find((r) => r.id === selected);
  useEffect(() => {
    setViewedMessageRoom(
      active && !inbox && !savedView && (!compact || !listVisible)
        ? selected
        : null,
    );
    return () => setViewedMessageRoom(null);
  }, [active, selected, inbox, savedView, compact, listVisible]);
  const selectRoom = (id: string) => {
    ++openEpoch.current;
    setOpening(false);
    setOpenError("");
    setSelected(id);
    setInbox(false);
    setSavedView(false);
    setJump(null);
    setListVisible(false);
  };
  return (
    <div className="messages-shell" ref={shell}>
      <div
        className={`team-messages${listVisible ? " shows-list" : " shows-detail"}`}
      >
        <MessageSidebar
          data={data}
          rooms={rooms}
          selected={selected}
          inbox={inbox}
          saved={savedView}
          onSaved={() => {
            ++openEpoch.current;
            setOpening(false);
            setOpenError("");
            setSavedView(true);
            setInbox(false);
            setListVisible(false);
          }}
          drafts={drafts}
          filter={search}
          onFilter={setSearch}
          onSelect={selectRoom}
          onCreate={setDialog}
          onMentions={() => {
            ++openEpoch.current;
            setOpening(false);
            setOpenError("");
            setInbox(true);
            setSavedView(false);
            setListVisible(false);
          }}
        />
        <div className="messages-detail">
          <div className="messages-compact-toolbar">
            <button onClick={() => setListVisible(true)}>
              <ChevronLeft size={17} /> Conversations
            </button>
          </div>
          {draftError && (
            <p className="messages-draft-error" role="alert">
              Draft could not be saved on this device. Keep this workspace open
              to retain it.
            </p>
          )}
          {openError && (
            <p className="team-error messages-open-error" role="alert">
              {openError}
            </p>
          )}
          {opening ? (
            <p className="messages-empty">Opening message…</p>
          ) : savedView ? (
            <SavedMessages
              org={org}
              user={user}
              onOpen={(id) => void openMessage(id)}
            />
          ) : inbox ? (
            <MentionsInbox
              org={org}
              user={user}
              onOpen={(id) => void openMessage(id)}
            />
          ) : current ? (
            <MessageRoom
              key={`${current.id}:${jump?.nonce ?? "normal"}`}
              jump={jump?.room.id === current.id ? jump : undefined}
              onSearch={onSearch}
              org={org}
              user={user}
              room={current}
              active={active && (!compact || !listVisible)}
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
              onOpenMessage={openMessage}
              onMeeting={onMeeting}
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
                  <h2>
                    {error ? "Chat is unavailable" : "Choose a conversation"}
                  </h2>
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
        </div>
        {dialog && (
          <NewConversation
            key={dialog}
            onKindChange={setDialog}
            data={data}
            kind={dialog}
            onClose={() => setDialog(null)}
            onCreated={(room) => {
              updateRoom(room);
              selectRoom(room.id);
              setSearch("");
              setDialog(null);
            }}
          />
        )}
      </div>
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
  onBackToThreads,
  onOpenMessage,
  onMeeting,
  jump,
  onSearch,
}: {
  jump?: TeamMessageLocation;
  onSearch?: (room: string) => void;
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
  // Present only when the thread was opened from the thread list.
  onBackToThreads?: () => void;
  onOpenMessage?: (id: string) => void;
  onMeeting?: (id: string) => void;
}) {
  const [unreadBoundary, setUnreadBoundary] = useState(
    room.first_unread_seq ?? 0,
  );
  const readState = useRef(room);
  readState.current = room;
  const [showPins, setShowPins] = useState(false);
  const [readBusy, setReadBusy] = useState(false);
  const positionKey = `${org}:${user}:${room.id}:${thread?.id ?? "main"}`;
  const destination = jump
    ? thread
      ? jump.message
      : (jump.parent ?? jump.message)
    : undefined;
  const restorePosition = useRef(
    destination
      ? { id: destination.id, seq: destination.created_seq, offset: 40 }
      : (readMessagePosition(positionKey) ??
          (!thread && room.first_unread_root_id
            ? {
                id: room.first_unread_root_id,
                seq: room.first_unread_seq ?? 0,
                offset: 40,
              }
            : undefined)),
  );
  const positionReady = useRef(false);
  const holdPosition = useRef(!!restorePosition.current);
  const wasActive = useRef(isActive);
  const historyContent = useRef<HTMLDivElement>(null);
  const rememberPosition = useCallback(() => {
    if (
      !positionReady.current ||
      !viewport.current ||
      !visible.current ||
      !viewport.current.clientHeight
    )
      return;
    const position = captureMessagePosition(viewport.current);
    if (position) saveMessagePosition(positionKey, position);
  }, [positionKey]);
  const draftKey = thread?.id ?? room.id;
  const [attachments, setAttachments] = useNavigationState<PendingAttachment[]>(
    `team:${org}:${user}:${draftKey}:attachments`,
    [],
  );
  // Same lifecycle as staged attachments: survives room and thread switches
  // and the jump remount, dropped on reload. Never the full message.
  const [replyTo, setReplyTo] = useNavigationState<TeamReplyReference | null>(
    `team:${org}:${user}:${draftKey}:reply-to`,
    null,
  );
  const [flash, setFlash] = useState<string | null>(null);
  const flashTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(flashTimer.current), []);
  const [readingFiles, setReadingFiles] = useState(false);
  const draft = drafts[draftKey] ?? "";
  const setDraft = (value: string) => setDraftFor(draftKey, value);
  const [threadRoot, setThreadRoot] = useState<TeamChatMessage | null>(
    jump?.parent ?? null,
  );
  // The thread-panel slot shows either the thread list or one thread; these
  // stay plain state because a jump remounts the room and discards them.
  const [threadList, setThreadList] = useState(false);
  const [threadFromList, setThreadFromList] = useState(false);
  const [threadListFocus, setThreadListFocus] = useState<string | null>(null);
  const [threadsVersion, setThreadsVersion] = useState(0);
  const threadsButton = useRef<HTMLButtonElement>(null);
  // Every exit from the panel goes through here so no caller forgets a piece.
  const resetThreadPanel = useCallback(() => {
    setThreadRoot(null);
    setThreadList(false);
    setThreadFromList(false);
    setThreadListFocus(null);
  }, []);
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
  const newerAfter = useRef<number | null>(null);
  const [hasNewer, setHasNewer] = useState(false);
  const [loadingNewer, setLoadingNewer] = useState(false);
  const [olderBefore, setOlderBefore] = useState<number | null>(null),
    [newBelow, setNewBelow] = useState(false);
  const [deleting, setDeleting] = useState<TeamChatMessage | null>(null);
  const [editing, setEditing] = useState<TeamChatMessage | null>(null),
    [settings, setSettings] = useState(false);
  const [retry, setRetry] = useState(0);
  const cursor = useRef<number | null>(null),
    readCursor = useRef(0);
  const viewport = useRef<HTMLDivElement>(null),
    composer = useRef<MessageComposerHandle>(null);
  const attachmentPicker = useRef<AttachmentPickerHandle>(null);
  const dragDepth = useRef(0);
  const [draggingFiles, setDraggingFiles] = useState(false);
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
  // Only rows in the loaded window are visible here; a target deleted outside
  // it is caught by the server's 409 at send time instead.
  useEffect(() => {
    if (replyTo && messages.find((m) => m.id === replyTo.id)?.deleted_at) {
      setReplyTo(null);
      setSendError("The message you were replying to was deleted.");
    }
  }, [messages, replyTo, setReplyTo]);
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
        readState.current.read_held ||
        !visible.current ||
        newerAfter.current != null ||
        !document.hasFocus() ||
        seq <= readCursor.current ||
        document.visibilityState !== "visible"
      )
        return;
      try {
        const version = readState.current.read_version;
        const result = await team.request<{ held?: boolean }>(
          "POST",
          `${path}/read`,
          { cursor: seq, version },
        );
        if (result.held || version !== readState.current.read_version) return;
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
    if (newerAfter.current != null) {
      ++accessEpoch.current;
      setMessages([]);
      newerAfter.current = null;
      setHasNewer(false);
      restorePosition.current = undefined;
      positionReady.current = false;
      cursor.current = null;
      setLoaded(false);
      setRetry((value) => value + 1);
    }
    holdPosition.current = false;
    pinned.current = true;
    setNewBelow(false);
    setScrollVersion((value) => value + 1);
  }, []);
  useLayoutEffect(() => {
    const el = viewport.current;
    if (!isActive) {
      wasActive.current = false;
      return;
    }
    if (!el || !loaded || error) return;
    if (!wasActive.current) {
      restorePosition.current = readMessagePosition(positionKey);
      positionReady.current = false;
      holdPosition.current = !!restorePosition.current;
    }
    wasActive.current = true;
    if (!positionReady.current) {
      const saved = restorePosition.current;
      if (saved && restoreMessagePosition(el, saved)) {
        pinned.current =
          !holdPosition.current &&
          el.scrollHeight - el.scrollTop - el.clientHeight < 8;
      } else {
        el.scrollTop = el.scrollHeight;
        pinned.current = true;
      }
      positionReady.current = true;
      restorePosition.current = undefined;
      setNewBelow(el.scrollHeight - el.scrollTop - el.clientHeight >= 8);
    } else if (prepend.current) {
      el.scrollTop =
        prepend.current.top + el.scrollHeight - prepend.current.height;
      prepend.current = null;
    } else if (pinned.current) {
      el.scrollTop = el.scrollHeight;
    }
    if (pinned.current && cursor.current != null)
      void acknowledge(cursor.current);
    rememberPosition();
  }, [
    messages,
    isActive,
    loaded,
    scrollVersion,
    acknowledge,
    parent,
    error,
    rememberPosition,
    positionKey,
  ]);
  useLayoutEffect(() => {
    const el = viewport.current;
    const content = historyContent.current;
    if (!el || !content) return;
    const observer = new ResizeObserver(() => {
      if (!visible.current || !positionReady.current) return;
      if (pinned.current) el.scrollTop = el.scrollHeight;
      rememberPosition();
    });
    observer.observe(el);
    observer.observe(content);
    return () => observer.disconnect();
  }, [rememberPosition]);
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
        if (initial && restorePosition.current)
          params.set("around", restorePosition.current.id);
        if (!initial) {
          params.set("after", String(after));
          params.set("wait", "20000");
        }
        let page: TeamChatPage;
        try {
          page = await team.request<TeamChatPage>(
            "GET",
            `${path}/messages?${params}`,
          );
        } catch (e) {
          if (!initial || !params.has("around") || destination) throw e;
          params.delete("around");
          page = await team.request<TeamChatPage>(
            "GET",
            `${path}/messages?${params}`,
          );
          restorePosition.current = undefined;
        }
        if (!active) return;
        if (epoch !== accessEpoch.current) {
          timer = window.setTimeout(poll, 1_000);
          return;
        }
        if (initial) {
          newerAfter.current = page.newer_after ?? null;
          setHasNewer(newerAfter.current != null);
        }
        readState.current = page.room;
        if (page.room.first_unread_seq)
          setUnreadBoundary((old) => old || page.room.first_unread_seq!);
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
                    (newerAfter.current == null && m.created_seq > after!) ||
                    old.some((existing) => existing.id === m.id),
                ),
              ),
        );
        if (initial) setOlderBefore(page.older_before);
        // Reply sends, edits and deletes re-emit their root, so an open
        // thread list refreshes within long-poll latency. quotedChanged()
        // fan-out re-emits quoting rows too, so a root with replies that is
        // also quoted can trigger one extra, harmless refetch.
        if (
          !initial &&
          !threadId &&
          page.messages.some((m) => m.thread_id || (m.reply_count ?? 0) > 0)
        )
          setThreadsVersion((v) => v + 1);
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
        resetThreadPanel();
        setProfile(null);
        setParent(undefined);
        setLoaded(true);
        cursor.current = null;
        newerAfter.current = null;
        setHasNewer(false);
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
  }, [path, onRoom, retry, isActive, threadId, resetThreadPanel]);
  const newer = async () => {
    if (newerAfter.current == null || loadingNewer) return;
    setLoadingNewer(true);
    const epoch = accessEpoch.current;
    try {
      const params = new URLSearchParams({ newer: String(newerAfter.current) });
      if (threadId) params.set("thread", threadId);
      const page = await team.request<TeamChatPage>(
        "GET",
        `${path}/messages?${params}`,
      );
      if (!alive.current || epoch !== accessEpoch.current) return;
      pinned.current = false;
      holdPosition.current = true;
      newerAfter.current = page.newer_after ?? null;
      setHasNewer(newerAfter.current != null);
      setMessages((old) => mergeMessages(old, page.messages));
      onRoom(page.room);
    } catch (e) {
      if (alive.current && epoch === accessEpoch.current) {
        ++accessEpoch.current;
        setMessages([]);
        setError(String(e));
        cursor.current = null;
        newerAfter.current = null;
        setHasNewer(false);
        resetThreadPanel();
        setParent(undefined);
        setProfile(null);
      }
    } finally {
      if (alive.current) setLoadingNewer(false);
    }
  };
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
        resetThreadPanel();
        setParent(undefined);
        setProfile(null);
        cursor.current = null;
        newerAfter.current = null;
        setHasNewer(false);
        readCursor.current = 0;
        setOlderBefore(null);
      }
    } finally {
      if (alive.current) setLoadingOlder(false);
    }
  };
  const send = async () => {
    const body = draft.trim();
    if (
      !isActive ||
      (!body && !attachments.length) ||
      sending ||
      readingFiles ||
      !canSend ||
      error
    )
      return;
    const replyToId = replyTo?.id ?? null;
    const clientId = sendKeyFor(
        draftKey,
        sendAttemptKey(
          body,
          attachments.map((f) => f.id),
          replyToId,
        ),
      ),
      epoch = accessEpoch.current;
    setSending(true);
    setSendError("");
    try {
      const message = await team.request<TeamChatMessage>(
        "POST",
        `${path}/messages`,
        {
          body,
          client_id: clientId,
          thread_id: threadId,
          reply_to_id: replyToId,
          attachments: attachments.map(({ name, data }) => ({ name, data })),
        },
      );
      onSentFor(draftKey, body);
      setAttachments([]);
      setReplyTo(null);
      if (!alive.current || epoch !== accessEpoch.current) return;
      setMessages((old) => mergeMessages(old, [message]));
      toBottom();
      requestAnimationFrame(() =>
        composer.current?.focus({ preventScroll: true }),
      );
    } catch (e) {
      if (!alive.current) return;
      setSendError(String(e));
      // A failed send keeps the bar so the draft can be retried as written,
      // except when the target itself is gone: that retry can never succeed.
      if (String(e).includes("replying to was deleted")) setReplyTo(null);
    } finally {
      if (alive.current) setSending(false);
    }
  };
  const quote = (message: TeamChatMessage) => {
    setReplyTo(replyReference(message));
    requestAnimationFrame(() =>
      composer.current?.focus({ preventScroll: true }),
    );
  };
  // In-window first so a jump keeps the loaded page, drafts and scroll state;
  // a target outside the window goes through the normal open flow (an
  // around= remount, thread panel reopened when the target is a reply).
  const jumpToMessage = (target: string) => {
    const el = viewport.current;
    if (el && restoreMessagePosition(el, { id: target, seq: 0, offset: 40 })) {
      // onScroll recomputes pinned from holdPosition, so both must change
      // before the scroll event lands or the next poll snaps to the bottom.
      holdPosition.current = true;
      pinned.current = false;
      setNewBelow(el.scrollHeight - el.scrollTop - el.clientHeight >= 8);
      setFlash(target);
      window.clearTimeout(flashTimer.current);
      flashTimer.current = window.setTimeout(
        () => setFlash((f) => (f === target ? null : f)),
        1600,
      );
    } else onOpenMessage?.(target);
  };
  const [caret, setCaret] = useState(0);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const eligible =
    room.kind === "direct"
      ? members.filter((m) =>
          room.participants.some((p) => p.id === m.id && p.active),
        )
      : members;
  const query = dismissed ? null : mentionQuery(draft, caret);
  const suggestions = query
    ? eligible
        .filter((m) =>
          m.name
            .toLocaleLowerCase()
            .startsWith(query.query.toLocaleLowerCase()),
        )
        .slice(0, 8)
    : [];
  const chooseMention = (member: TeamUser) => {
    if (!query) return;
    const insertion = `@${member.name} `;
    const next =
      draft.slice(0, query.start) + insertion + draft.slice(query.end);
    if (next.length > 10_000) return;
    setDraft(next);
    setDismissed(true);
    requestAnimationFrame(() => {
      composer.current?.focus();
      composer.current?.setSelectionRange(
        query.start + insertion.length,
        query.start + insertion.length,
      );
    });
  };
  // Passed to MessageRow so the row itself stays unaware of mentions.
  const renderPlainBody = (body: string) => {
    let end = 0;
    const parts = findMentions(body, eligible).map((mention) => {
      const before = body.slice(end, mention.start);
      end = mention.end;
      return (
        <Fragment key={mention.start}>
          {before}
          <mark
            className={
              mention.user.id === user
                ? "messages-mention self"
                : "messages-mention"
            }
          >
            {body.slice(mention.start, mention.end)}
          </mark>
        </Fragment>
      );
    });
    return (
      <>
        {parts}
        {body.slice(end)}
      </>
    );
  };
  const renderBody = (body: string) => {
    let end = 0;
    const parts = messageFormatting(body).map((span) => {
      const before = body.slice(end, span.start);
      end = span.end;
      const content = body.slice(span.contentStart, span.contentEnd);
      return (
        <Fragment key={span.start}>
          {renderPlainBody(before)}
          <span className={`message-format-${span.kind}`}>
            {span.kind === "code" ? content : renderPlainBody(content)}
          </span>
        </Fragment>
      );
    });
    return (
      <>
        {parts}
        {renderPlainBody(body.slice(end))}
      </>
    );
  };
  const label = roomLabel(room, user);
  const unreadThreads = room.unread_threads ?? 0;
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
        highlighted={destination?.id === message.id || flash === message.id}
        person={person}
        onMarkUnread={
          room.unread_navigation
            ? async () => {
                if (readBusy) return;
                setReadBusy(true);
                try {
                  const updated = await team.request<TeamChatRoom>(
                    "POST",
                    `${path}/unread`,
                    { message_id: message.id },
                  );
                  readState.current = updated;
                  onRoom(updated);
                  setUnreadBoundary(message.created_seq);
                } catch (e) {
                  setSendError(String(e));
                } finally {
                  setReadBusy(false);
                }
              }
            : undefined
        }
        canSend={canSend}
        renderBody={renderBody}
        showReplies={showReplies}
        extras={!!room.message_extras}
        pinsEnabled={!!room.pins_enabled}
        savedEnabled={!!room.saved_messages_enabled}
        onMeeting={onMeeting}
        onChanged={changed}
        onReply={() =>
          threadId ? composer.current?.focus() : setThreadRoot(message)
        }
        // The thread panel's parent row sits at the other level, so it never
        // gets the action; an older server without the flag shows nothing.
        onQuote={
          room.inline_replies && canSend && canReplyInline(message, threadId)
            ? () => quote(message)
            : undefined
        }
        onOpenQuoted={jumpToMessage}
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
      // Escape precedence, outermost last: autocomplete picker → cancel
      // pending reply (both in the composer onKeyDown, which stops
      // propagation) → back to threads → close thread. An open <dialog>
      // suppresses all of them.
      onKeyDown={(event) => {
        if (
          threadId &&
          event.key === "Escape" &&
          !document.querySelector("dialog[open]")
        ) {
          event.stopPropagation();
          (onBackToThreads ?? onCloseThread)?.();
        }
      }}
    >
      <div
        className="messages-room-main"
        inert={!!threadRoot || threadList || undefined}
        onDragEnter={(event) => {
          if (!event.dataTransfer.types.includes("Files")) return;
          event.preventDefault();
          dragDepth.current++;
          if (room.attachments_enabled && canSend && !sending && !readingFiles)
            setDraggingFiles(true);
        }}
        onDragOver={(event) => {
          if (!event.dataTransfer.types.includes("Files")) return;
          event.preventDefault();
          event.dataTransfer.dropEffect =
            room.attachments_enabled && canSend && !sending && !readingFiles
              ? "copy"
              : "none";
        }}
        onDragLeave={(event) => {
          if (!event.dataTransfer.types.includes("Files")) return;
          dragDepth.current = Math.max(0, dragDepth.current - 1);
          if (!dragDepth.current) setDraggingFiles(false);
        }}
        onDrop={(event) => {
          if (!event.dataTransfer.types.includes("Files")) return;
          event.preventDefault();
          event.stopPropagation();
          dragDepth.current = 0;
          setDraggingFiles(false);
          if (room.attachments_enabled && canSend && !sending && !readingFiles)
            attachmentPicker.current?.addFiles(
              Array.from(event.dataTransfer.files),
            );
        }}
      >
        {draggingFiles && (
          <div className="message-drop-target" role="status">
            <strong>Drop files to attach</strong>
            <small>Up to 3 files · 5 MiB total · Send when you’re ready</small>
          </div>
        )}
        {room.read_held && (
          <div className="message-unread-notice" role="status">
            <span>Marked unread · Kept unread until you mark it read</span>
            <button
              className="team-text-button"
              disabled={readBusy}
              onClick={async () => {
                setReadBusy(true);
                try {
                  await team.request("POST", `${path}/read`, {
                    cursor: room.notification_cursor ?? 0,
                    version: room.read_version,
                    resume: true,
                  });
                  const updated = await team.request<TeamChatRoom>("GET", path);
                  readState.current = updated;
                  onRoom(updated);
                  setUnreadBoundary(0);
                  onRead(id);
                } catch (e) {
                  setSendError(String(e));
                } finally {
                  setReadBusy(false);
                }
              }}
            >
              Mark read
            </button>
          </div>
        )}
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
          {threadId && onBackToThreads && (
            <button
              className="team-text-button"
              aria-label="Back to threads"
              title="Back to threads"
              onClick={onBackToThreads}
            >
              <ChevronLeft size={18} />
            </button>
          )}
          {threadId && (
            <button
              className="team-text-button"
              aria-label="Close thread"
              onClick={onCloseThread}
            >
              <X size={18} />
            </button>
          )}
          {!threadId && room.unread_navigation && room.first_unread_id && (
            <button
              className="team-text-button"
              onClick={() => onOpenMessage?.(room.first_unread_id!)}
            >
              Jump to unread
            </button>
          )}
          {!threadId && room.pins_enabled && (
            <button
              className="team-text-button"
              onClick={() => setShowPins(true)}
            >
              Pinned
            </button>
          )}
          {!threadId && room.threads_enabled && (
            <button
              ref={threadsButton}
              className="team-text-button"
              aria-expanded={threadList}
              aria-controls={threadList ? `threads-${room.id}` : undefined}
              aria-label={
                unreadThreads
                  ? `Threads, ${unreadThreads} with new replies`
                  : "Threads"
              }
              onClick={() => {
                const open = !threadList;
                resetThreadPanel();
                setThreadList(open);
              }}
            >
              <MessagesSquare size={17} /> Threads
              {unreadThreads > 0 && (
                <span className="messages-head-count">{unreadThreads}</span>
              )}
            </button>
          )}
          {!threadId && onSearch && (
            <button
              className="team-text-button"
              aria-label="Search this conversation"
              title="Search this conversation"
              onClick={() => onSearch(room.id)}
            >
              <Search size={17} />
            </button>
          )}
          {!threadId && (
            <ConversationNotifications org={org} room={room} onSaved={onRoom} />
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
          onWheel={() => {
            holdPosition.current = false;
          }}
          onTouchMove={() => {
            holdPosition.current = false;
          }}
          onPointerDown={() => {
            holdPosition.current = false;
          }}
          onKeyDown={() => {
            holdPosition.current = false;
          }}
          onScroll={() => {
            const el = viewport.current;
            if (!el || !positionReady.current || !isActive || !el.clientHeight)
              return;
            pinned.current =
              !holdPosition.current &&
              el.scrollHeight - el.scrollTop - el.clientHeight < 8;
            if (pinned.current) {
              setNewBelow(false);
              if (cursor.current != null) void acknowledge(cursor.current);
            }
            rememberPosition();
          }}
        >
          <div ref={historyContent}>
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
                  {unreadBoundary > 0 &&
                    message.created_seq >= unreadBoundary &&
                    !messages
                      .slice(0, index)
                      .some((m) => m.created_seq >= unreadBoundary) && (
                      <div className="message-new-divider">New messages</div>
                    )}
                  {messageRow(message, !threadId)}
                </Fragment>
              );
            })}
            {hasNewer && (
              <button
                className="team-text-button messages-older"
                disabled={loadingNewer}
                onClick={() => void newer()}
              >
                {loadingNewer ? "Loading…" : "Load later messages"}
              </button>
            )}
          </div>
        </div>
        {(newBelow || hasNewer) && (
          <button className="messages-new team-primary" onClick={toBottom}>
            <ArrowDown size={14} />{" "}
            {hasNewer ? "Jump to latest" : "New messages"}
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
            {replyTo
              ? `Reply to ${replyTo.author_name}`
              : threadId
                ? "Reply in thread"
                : room.kind === "channel" && !room.is_default
                  ? `Message #${label}`
                  : `Message ${label}`}
          </label>
          {/* Always rendered so assistive tech announces the bar appearing and
              leaving; the composer's name and description point at it too. */}
          <div className="messages-compose-reply-live" aria-live="polite">
            {replyTo && (
              <div
                className="messages-compose-reply"
                id={`reply-to-${draftKey}`}
              >
                <CornerUpLeft size={13} aria-hidden="true" />
                <span>
                  Replying to <b>{replyTo.author_name}</b>
                  {quotePreview(replyTo) ? ` · ${quotePreview(replyTo)}` : ""}
                </span>
                <button
                  type="button"
                  className="team-text-button"
                  aria-label="Cancel reply"
                  title="Cancel reply · Escape"
                  onClick={() => {
                    setReplyTo(null);
                    composer.current?.focus({ preventScroll: true });
                  }}
                >
                  <X size={14} />
                </button>
              </div>
            )}
          </div>
          {suggestions.length > 0 && (
            <div
              className="messages-mention-picker"
              id={`mentions-${draftKey}`}
              role="listbox"
              aria-label="Mention a teammate"
            >
              {suggestions.map((member, index) => (
                <button
                  type="button"
                  role="option"
                  id={`mention-${draftKey}-${index}`}
                  aria-selected={index === mentionIndex % suggestions.length}
                  key={member.id}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => chooseMention(member)}
                >
                  @{member.name}
                </button>
              ))}
            </div>
          )}
          <div className="messages-compose-box">
          {room.attachments_enabled && (
            <AttachmentPicker
              ref={attachmentPicker}
              files={attachments}
              onChange={setAttachments}
              disabled={sending || !canSend}
              onBusy={setReadingFiles}
            />
          )}
          <MessageComposer
            aria-controls={
              suggestions.length ? `mentions-${draftKey}` : undefined
            }
            aria-activedescendant={
              suggestions.length
                ? `mention-${draftKey}-${mentionIndex % suggestions.length}`
                : undefined
            }
            aria-describedby={replyTo ? `reply-to-${draftKey}` : undefined}
            id={`compose-${draftKey}`}
            ref={composer}
            value={draft}
            disabled={sending || !canSend}
            placeholder={
              canSend
                ? replyTo
                  ? `Reply to ${replyTo.author_name}`
                  : threadId
                    ? "Reply in thread"
                    : `Message ${room.kind === "channel" && !room.is_default ? "#" : ""}${label}`
                : "This conversation is read-only"
            }
            onChange={(value, caret) => {
              setDraft(value);
              setCaret(caret);
              setMentionIndex(0);
              setDismissed(false);
            }}
            onSelect={setCaret}
            onKeyDown={(e) => {
              if (e.isComposing) return;
              if (suggestions.length) {
                if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  setDismissed(true);
                  return;
                }
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  setMentionIndex(
                    (old) =>
                      (old +
                        (e.key === "ArrowDown" ? 1 : suggestions.length - 1)) %
                      suggestions.length,
                  );
                  return;
                }
                if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
                  e.preventDefault();
                  chooseMention(suggestions[mentionIndex % suggestions.length]);
                  return;
                }
              }
              if (e.key === "Escape" && replyTo) {
                e.preventDefault();
                // Without this the section handler would also back out of or
                // close the thread on the same keypress.
                e.stopPropagation();
                setReplyTo(null);
                return;
              }
              if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
                e.preventDefault();
                void send();
              }
            }}
          />
          <div className="messages-compose-bar">
            {room.attachments_enabled && (
              <button
                type="button"
                className="icon-btn messages-compose-attach"
                disabled={
                  sending || readingFiles || !canSend || attachments.length >= 3
                }
                onClick={() => attachmentPicker.current?.open()}
                aria-label="Attach files"
                title="Attach files · PNG, JPEG, PDF, or text · 5 MiB total"
              >
                {readingFiles ? (
                  <Loader size={15} className="spin" />
                ) : (
                  <Paperclip size={15} />
                )}
              </button>
            )}
            <button
              className="messages-compose-send"
              disabled={
                sending ||
                readingFiles ||
                !canSend ||
                (!draft.trim() && !attachments.length) ||
                !!error
              }
              aria-label={threadId ? "Reply" : "Send"}
              title={threadId ? "Reply · Enter" : "Send · Enter"}
            >
              {sending ? (
                <Loader size={14} className="spin" />
              ) : (
                <Send size={14} />
              )}
            </button>
          </div>
          </div>
          <p className="messages-compose-hint">
            Markdown · @ to mention · Enter to send · Shift + Enter for a new line
          </p>
        </form>
      </div>
      {showPins && (
        <PinnedMessages
          org={org}
          room={room.id}
          onClose={() => setShowPins(false)}
          onOpen={(id) => onOpenMessage?.(id)}
        />
      )}
      {(threadRoot || threadList) && !threadId && (
        <div className="messages-thread-panel">
          {threadRoot ? (
            <MessageRoom
              key={threadRoot.id}
              org={org}
              user={user}
              room={room}
              active={isActive}
              memberCount={memberCount}
              members={members}
              thread={threadRoot}
              jump={jump?.parent?.id === threadRoot.id ? jump : undefined}
              drafts={drafts}
              setDraftFor={setDraftFor}
              sendKeyFor={sendKeyFor}
              onSentFor={onSentFor}
              onRoom={onRoom}
              onRead={onRead}
              onOpenMessage={onOpenMessage}
              onMeeting={onMeeting}
              onBackToThreads={
                threadFromList
                  ? () => {
                      setThreadListFocus(threadRoot.id);
                      setThreadRoot(null);
                      setThreadList(true);
                    }
                  : undefined
              }
              onCloseThread={() => {
                resetThreadPanel();
                requestAnimationFrame(() =>
                  composer.current?.focus({ preventScroll: true }),
                );
              }}
            />
          ) : (
            <ThreadList
              id={`threads-${room.id}`}
              org={org}
              user={user}
              room={room}
              active={isActive}
              version={threadsVersion}
              focusRoot={threadListFocus}
              onOpen={(root) => {
                setThreadFromList(true);
                setThreadList(false);
                setThreadRoot(root);
              }}
              onClose={() => {
                resetThreadPanel();
                requestAnimationFrame(() =>
                  threadsButton.current?.focus({ preventScroll: true }),
                );
              }}
            />
          )}
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
