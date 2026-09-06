import { expect, test } from "bun:test";
import { mergeMessages } from "../src/teams/messaging";
import type { TeamChatMessage } from "../src/teams/types";

const message = (
  id: string,
  seq: number,
  revision = 1,
  body = "Hello",
  overrides: Partial<TeamChatMessage> = {},
): TeamChatMessage => ({
  id,
  created_seq: seq,
  revision,
  body,
  room_id: "room",
  author_id: "user",
  author_name: "Taylor",
  created_at: "2026-09-05T00:00:00Z",
  edited_at: null,
  deleted_at: null,
  can_edit: true,
  can_delete: true,
  ...overrides,
});
test("out-of-order history and send acknowledgments cannot replace newer edits or resurrect deletions", () => {
  const deleted = {
    ...message("first", 1, 3, ""),
    deleted_at: "2026-09-05T00:01:00Z",
  };
  const rows = mergeMessages(
    [message("second", 2, 2, "Edited"), deleted],
    [message("second", 2), message("first", 1), message("third", 3)],
  );
  expect(rows.map((m) => m.id)).toEqual(["first", "second", "third"]);
  expect(rows[0].deleted_at).not.toBeNull();
  expect(rows[0].body).toBe("");
  expect(rows[1].body).toBe("Edited");
  // Fan-out re-emits a quoting row at an unchanged revision with a fresh
  // reply_to; equal revisions must replace, lower ones must not.
  const quoted = { id: "a", author_id: "u", author_name: "Taylor", body: "Old", deleted_at: null, created_seq: 1 };
  const refreshed = { ...quoted, body: "", deleted_at: "2026-09-05T00:02:00Z" };
  const live = mergeMessages(
    [message("reply", 4, 2, "Sure", { reply_to_id: "a", reply_to: quoted })],
    [message("reply", 4, 2, "Sure", { reply_to_id: "a", reply_to: refreshed })],
  );
  expect(live[0].reply_to?.deleted_at).not.toBeNull();
  const stale = mergeMessages(
    [message("reply", 4, 2, "Sure", { reply_to_id: "a", reply_to: refreshed })],
    [message("reply", 4, 1, "Sure", { reply_to_id: "a", reply_to: quoted })],
  );
  expect(stale[0].reply_to?.deleted_at).not.toBeNull();
});

import { canReplyInline, quotePreview, replyReference, sendAttemptKey } from "../src/teams/messaging";

test("inline reply helpers key sends by target, preview quotes on one line and mirror the level rule", () => {
  expect(sendAttemptKey("Hi", [], null)).toBe("Hi");
  const keys = [sendAttemptKey("Hi", [], "a"), sendAttemptKey("Hi", [], "b"), sendAttemptKey("Hi", ["f"], null), sendAttemptKey("Hi", ["f"], "a")];
  expect(new Set([...keys, "Hi"]).size).toBe(5);
  const original = message("a", 1, 1, "First line\n\n  second   line ".padEnd(200, "x"));
  const ref = replyReference(original);
  expect(ref).toMatchObject({ id: "a", author_id: "user", author_name: "Taylor", deleted_at: null, created_seq: 1 });
  expect(ref.body).toHaveLength(160);
  expect(quotePreview(ref)).toMatch(/^First line second line x+…$/);
  expect(quotePreview(ref)).toHaveLength(121);
  expect(quotePreview(replyReference(message("b", 2, 1, "Short")))).toBe("Short");
  expect(replyReference(message("f", 3, 1, "", { attachments: [{ id: "x", name: "a.png", mime: "image/png", size: 1 }] })).body).toBe("Shared an attachment or meeting");
  expect(quotePreview({ ...ref, body: "", deleted_at: "2026-09-05T00:01:00Z" })).toBe("Original message deleted");
  expect(quotePreview(null)).toBe("");
  expect(quotePreview(undefined)).toBe("");
  expect(canReplyInline(message("m", 4), undefined)).toBe(true);
  expect(canReplyInline(message("t", 5, 1, "Hi", { thread_id: "root" }), "root")).toBe(true);
  expect(canReplyInline(message("d", 6, 1, "", { deleted_at: "2026-09-05T00:01:00Z" }), undefined)).toBe(false);
  expect(canReplyInline(message("t", 7, 1, "Hi", { thread_id: "root" }), undefined)).toBe(false);
  expect(canReplyInline(message("root", 8, 1, "Hi", { thread_id: null }), "root")).toBe(false);
});

import { bodyMentions, channelQuery, findChannelMentions, findMentions, mentionQuery } from "../src/teams/mentions";
import { draftKey, readDrafts, writeDrafts } from "../src/teams/messageDrafts";

test("channel references resolve only exact visible names and never inside words or URLs", () => {
  const channels = [
    { id: "d", name: "design" },
    { id: "g", name: "general" },
    { id: "a", name: "old-topic", archived_at: "2026-09-01T00:00:00Z" },
  ];
  const body = "see #design, #Design. (#general) #design-system #nope C#design issue#12 ##design https://x/p#design https://x.test/#design # heading";
  expect(findChannelMentions(body, channels).map((h) => [h.room.id, h.start, h.end])).toEqual([["d", 4, 11], ["d", 13, 20], ["g", 23, 31]]);
  expect(findChannelMentions("back in #old-topic", channels).map((h) => h.room.id)).toEqual(["a"]);
  expect(findChannelMentions("#design-", channels)).toEqual([]);
  expect(findChannelMentions("", channels)).toEqual([]);
  expect(findChannelMentions("#", channels)).toEqual([]);
  expect(findChannelMentions("#design", [])).toEqual([]);
});

test("channel query opens on # after whitespace, filters without spaces, and takes precedence over a mention query", () => {
  expect(channelQuery("see #des", 8)).toEqual({ start: 4, end: 8, query: "des" });
  expect(channelQuery("#", 1)).toEqual({ start: 0, end: 1, query: "" });
  expect(channelQuery("a#b", 3)).toBeNull();
  expect(channelQuery("#des ign", 8)).toBeNull();
  expect(channelQuery("# title", 7)).toBeNull();
  // mentionQuery's [^@\n]* swallows "#de", so the component must evaluate
  // channelQuery first for "@Ed #de" to reach the channel picker.
  expect(channelQuery("@Ed #de", 7)).toEqual({ start: 4, end: 7, query: "de" });
  expect(mentionQuery("@Ed #de", 7)).not.toBeNull();
  expect(channelQuery("#design @Ed", 11)).toBeNull();
  expect(mentionQuery("#design @Ed", 11)?.query).toBe("Ed");
});

test("body mentions merge people and channels in order and drop overlaps", () => {
  const members = [{ id: "a", name: "Edison Chen" }, { id: "b", name: "Ed Smith" }];
  const channels = [{ id: "d", name: "design" }];
  const hits = bodyMentions("@Edison see #design and @Ed Smith", members, channels);
  expect(hits.map((h) => h.kind)).toEqual(["member", "channel", "member"]);
  for (let i = 1; i < hits.length; i++) expect(hits[i].start).toBeGreaterThanOrEqual(hits[i - 1].end);
  const overlap = bodyMentions("@Team #1", [{ id: "t", name: "Team #1" }], [{ id: "one", name: "1" }]);
  expect(overlap).toHaveLength(1);
  expect(overlap[0].kind).toBe("member");
});

test("mentions match full names and unique first names, avoiding emails and ambiguous pings", () => {
  const members = [{ id: "a", name: "Edison Chen" }, { id: "b", name: "Ed Smith" }];
  expect(findMentions("@Edison, ask @Ed Smith. mail@Edison.com @Edisonx", members).map((m) => m.user.id)).toEqual(["a", "b"]);
  expect(findMentions("@Edison", [...members, { id: "c", name: "Edison Lee" }])).toEqual([]);
  expect(findMentions("@Edison Chen", [...members, { id: "c", name: "Edison Lee" }])[0].user.id).toBe("a");
  expect(mentionQuery("Hi @Edi later", 7)).toEqual({ start: 3, end: 7, query: "Edi" });
  expect(mentionQuery("hi@Edi", 6)).toBeNull();
});

test("drafts restore independently by account and room and remove only cleared drafts", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  } });
  try {
    const key = draftKey("org", "alice");
    expect(writeDrafts(key, { first: "Unsent\ntext", second: "Other" })).toBe(true);
    expect(readDrafts(key)).toEqual({ first: "Unsent\ntext", second: "Other" });
    expect(readDrafts(draftKey("org", "bob"))).toEqual({});
    expect(readDrafts(draftKey("other", "alice"))).toEqual({});
    writeDrafts(key, { ...readDrafts(key), first: "" });
    expect(readDrafts(key)).toEqual({ second: "Other" });
    values.set(key, "invalid JSON");
    expect(readDrafts(key)).toEqual({});
  } finally {
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});

import { MentionNotificationTracker } from "../src/teams/mentionNotifications";
import type { TeamChatRoom } from "../src/teams/types";

test("desktop mentions baseline history, deduplicate polls and isolate recipients", () => {
  const tracker = new MentionNotificationTracker();
  const room = (cursor: number, mention: number, user = "alice", archived = false) => ({
    id: "room", notification_cursor: cursor, latest_unread_mention_seq: mention,
    notification_user_id: user, archived_at: archived ? "now" : null,
  }) as TeamChatRoom;
  expect(tracker.update("server:org", [room(10, 9)])).toHaveLength(0);
  expect(tracker.update("server:org", [room(11, 11)])).toHaveLength(1);
  expect(tracker.update("server:org", [room(11, 11)])).toHaveLength(0);
  expect(tracker.update("server:org", [room(12, 0)])).toHaveLength(0);
  expect(tracker.update("server:org", [room(13, 11)])).toHaveLength(0);
  expect(tracker.update("server:org", [room(14, 14, "bob")])).toHaveLength(0);
  expect(tracker.update("server:org", [room(15, 15, "alice", true)])).toHaveLength(0);
  expect(tracker.update("server:org", [room(15, 15)])).toHaveLength(0);
  expect(tracker.update("other:org", [room(20, 20)])).toHaveLength(0);
});

test("message alerts include ordinary messages without replaying edits or reads", () => {
  const tracker = new MentionNotificationTracker();
  const room = (cursor: number, incoming: number) => ({
    id: "dm", notification_user_id: "recipient", notification_cursor: cursor,
    latest_unread_message_seq: incoming, latest_unread_mention_seq: 0,
    archived_at: null,
  }) as TeamChatRoom;
  expect(tracker.update("org", [room(10, 10)], "messages")).toHaveLength(0);
  expect(tracker.update("org", [room(11, 11)], "messages")).toHaveLength(1);
  expect(tracker.update("org", [room(12, 11)], "messages")).toHaveLength(0);
  expect(tracker.update("org", [room(12, 0)], "messages")).toHaveLength(0);
  expect(tracker.update("org", [room(13, 13)], "mentions")).toHaveLength(0);
});

import { captureMessagePosition, restoreMessagePosition, saveMessagePosition, readMessagePosition } from "../src/teams/messageScroll";

test("scroll anchors preserve a partially visible message when history and viewport change", () => {
  let scrollTop = 120;
  let contentShift = 0;
  const rows = [0, 100, 200].map((offset, index) => ({
    dataset: { messageId: `m${index}`, messageSeq: String(index + 1) },
    getBoundingClientRect: () => ({ top: 50 + offset + contentShift - scrollTop, bottom: 150 + offset + contentShift - scrollTop }),
  }));
  const viewport = {
    get scrollTop() { return scrollTop; },
    set scrollTop(value: number) { scrollTop = value; },
    getBoundingClientRect: () => ({ top: 50 }),
    querySelectorAll: () => rows,
  } as unknown as HTMLElement;
  const anchor = captureMessagePosition(viewport)!;
  expect(anchor).toEqual({ id: "m1", seq: 2, offset: -20 });
  saveMessagePosition("org:alice:dm:main", anchor);
  expect(readMessagePosition("org:bob:dm:main")).toBeUndefined();
  contentShift = 400; // Older history was added above the saved message.
  scrollTop = 0;
  expect(restoreMessagePosition(viewport, anchor)).toBe(true);
  expect(scrollTop).toBe(520);
  expect(captureMessagePosition(viewport)).toEqual(anchor);
  expect(restoreMessagePosition(viewport, { ...anchor, id: "missing" })).toBe(false);
});

import { conversationAlertMode } from "../src/teams/mentionNotifications";
test("conversation overrides support mute, mentions, defaults and unmute without old alerts", () => {
  const tracker = new MentionNotificationTracker();
  const room = (cursor: number, incoming: number, mention: number, mode: TeamChatRoom["notification_mode"]) => ({
    id: "room", notification_user_id: "recipient", notification_cursor: cursor,
    latest_unread_message_seq: incoming, latest_unread_mention_seq: mention, notification_mode: mode,
    archived_at: null,
  }) as TeamChatRoom;
  const effective = (r: TeamChatRoom) => conversationAlertMode(r, "messages");
  expect(tracker.update("org", [room(10, 10, 0, "default")], effective)).toHaveLength(0);
  expect(tracker.update("org", [room(11, 11, 11, "none")], effective)).toHaveLength(0);
  expect(tracker.update("org", [room(11, 11, 11, "messages")], effective)).toHaveLength(0);
  expect(tracker.update("org", [room(12, 12, 11, "mentions")], effective)).toHaveLength(0);
  expect(tracker.update("org", [room(13, 13, 13, "mentions")], effective)).toHaveLength(1);
  expect(tracker.update("org", [room(14, 14, 13, "default")], effective)).toHaveLength(1);
  expect(conversationAlertMode(room(14, 14, 13, "messages"), "mentions")).toBe("messages");
  expect(conversationAlertMode(room(14, 14, 13, "default"), "mentions")).toBe("mentions");
});

import { messagePreview, shortTime } from "../src/teams/messaging";
import { mergeThreads, threadParticipants } from "../src/teams/threads";
import type { TeamThreadSummary, TeamUser } from "../src/teams/types";

test("thread summaries merge by root, label participants and preview roots", () => {
  const taylor = { id: "t", name: "Taylor" }, alex = { id: "a", name: "Alex" }, me = { id: "me", name: "Me" };
  const thread = (root: TeamChatMessage, lastSeq: number, participants: TeamUser[] = [taylor]): TeamThreadSummary => ({
    root, reply_count: 1, unread_replies: 0, last_reply_seq: lastSeq, last_reply_at: "2026-09-05T00:00:00Z",
    last_reply_by: participants[0], participants, participant_count: participants.length,
  });
  const merged = mergeThreads(
    [thread(message("a", 1), 10), thread(message("b", 2), 20)],
    [thread(message("a", 1), 30), thread(message("b", 2), 15), thread(message("c", 3), 25)],
  );
  expect(merged.map((t) => [t.root.id, t.last_reply_seq])).toEqual([["a", 30], ["c", 25], ["b", 20]]);
  expect(mergeThreads([thread(message("a", 1), 10)], [thread(message("a", 1), 10, [alex])])[0].participants).toEqual([alex]);
  expect(threadParticipants([taylor], 1, "me")).toBe("Taylor");
  expect(threadParticipants([taylor, me], 2, "me")).toBe("Taylor and you");
  expect(threadParticipants([me, taylor, alex], 3, "me")).toBe("Taylor, Alex and you");
  expect(threadParticipants([taylor, alex], 5, "me")).toBe("Taylor, Alex and 3 others");
  expect(threadParticipants([taylor], 2, "me")).toBe("Taylor and 1 other");
  expect(threadParticipants([], 0, "me")).toBe("");
  expect(messagePreview(message("d", 4, 1, "", { deleted_at: "2026-09-05T00:01:00Z" }))).toBe("Message deleted");
  expect(messagePreview(message("f", 5, 1, "", { attachments: [{ id: "x", name: "a.png", mime: "image/png", size: 1 }] }))).toBe("Shared an attachment or meeting");
  expect(messagePreview(message("m", 6, 1, "", { has_meeting: true }))).toBe("Shared an attachment or meeting");
  expect(messagePreview(message("r", 7, 1, "Plain body", { thread_id: null, reply_count: 2, last_reply_at: null }))).toBe("Plain body");
  const now = new Date(2026, 8, 5, 20, 0);
  expect(shortTime(new Date(2026, 8, 5, 14, 30).toISOString(), now)).toMatch(/2:30/);
  expect(shortTime(new Date(2026, 8, 4, 14, 30).toISOString(), now)).toMatch(/^Sep 4$/);
});

import { slashCommands } from "../src/teams/composerCommands";
import { mediaCountLabel, mediaKey, mergeMedia } from "../src/teams/media";
import type { TeamMediaItem } from "../src/teams/types";

test("the document action is listed only when available and /doc filters to it", () => {
  const all = ["attach", "meeting", "document"] as const;
  expect(slashCommands("/", 1, [...all]).map((a) => a.id)).toEqual([...all]);
  expect(slashCommands("/doc", 4, [...all]).map((a) => a.id)).toEqual(["document"]);
  expect(slashCommands("/share", 6, [...all]).map((a) => a.id)).toEqual(["document"]);
  expect(slashCommands("/doc", 4, ["attach", "meeting"])).toEqual([]);
  expect(slashCommands("/", 1, ["attach", "meeting"]).map((a) => a.id)).toEqual(["attach", "meeting"]);
});

test("a staged document reference joins the retry key by id and revision alone, like a meeting", () => {
  const shared = { id: "n", revision: 3, title: "Roadmap", occurred_at: "2026-09-05T00:00:00Z", collection: "Product" };
  const document = sendAttemptKey("See", ["f"], null, { ...shared, kind: "document" as const });
  expect(document).toBe(sendAttemptKey("See", ["f"], null, { ...shared, kind: "meeting" as const }));
  expect(document).toBe(sendAttemptKey("See", ["f"], null, { id: "n", revision: 3 }));
  expect(document).not.toBe(sendAttemptKey("See", ["f"], null, { id: "n", revision: 4 }));
  expect(document).not.toBe(sendAttemptKey("See", ["f"], null, null));
});

test("media pages merge by attachment or document key, page one owning its range", () => {
  const author = { id: "t", name: "Taylor" };
  const file = (id: string, seq: number): TeamMediaItem => ({
    message_id: `m${seq}`, created_seq: seq, created_at: "2026-09-05T00:00:00Z", author,
    attachment: { id, name: `${id}.png`, mime: "image/png", size: 1 },
  });
  const doc = (note: string, seq: number): TeamMediaItem => ({
    message_id: `m${seq}`, created_seq: seq, created_at: "2026-09-05T00:00:00Z", author,
    document: { note_id: note, title: "Roadmap", updated: false },
  });
  // Two attachments on one message and one document shared twice stay distinct.
  expect(new Set([file("a", 9), file("b", 9), doc("n", 8), doc("n", 7)].map(mediaKey)).size).toBe(4);
  const old = [file("c", 30), file("b", 20), file("a", 10)];
  // Refresh: c's message was deleted, d is new; a is older than page one and is kept.
  const refreshed = mergeMedia(old, { items: [file("d", 40), file("b", 20)], next_before: 20 }, true);
  expect(refreshed.map((i) => i.attachment!.id)).toEqual(["d", "b", "a"]);
  // A complete page one (no continuation) replaces everything.
  expect(mergeMedia(old, { items: [file("b", 20)], next_before: null }, true).map((i) => i.attachment!.id)).toEqual(["b"]);
  // Load older appends only unseen rows and keeps order.
  const older = mergeMedia(refreshed, { items: [file("a", 10), file("z", 5)], next_before: null }, false);
  expect(older.map((i) => i.attachment!.id)).toEqual(["d", "b", "a", "z"]);
  expect(mediaCountLabel("images", 1)).toBe("1 image");
  expect(mediaCountLabel("documents", 2)).toBe("2 documents");
  expect(mediaCountLabel("files", 0)).toBe("0 files");
});
