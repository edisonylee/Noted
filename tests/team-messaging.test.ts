import { expect, test } from "bun:test";
import { mergeMessages } from "../src/teams/messaging";
import type { TeamChatMessage } from "../src/teams/types";

const message = (
  id: string,
  seq: number,
  revision = 1,
  body = "Hello",
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
