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
});

import { findMentions, mentionQuery } from "../src/teams/mentions";
import { draftKey, readDrafts, writeDrafts } from "../src/teams/messageDrafts";

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
