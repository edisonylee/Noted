import { expect, test } from "bun:test";
import { slashCommands } from "../src/teams/composerCommands";
import { sendAttemptKey } from "../src/teams/messaging";

test("slash commands are leading, capability scoped, and leave ordinary Markdown alone", () => {
  expect(slashCommands("/", 1, ["attach", "meeting"]).map((a) => a.id)).toEqual(
    ["attach", "meeting"],
  );
  expect(
    slashCommands("/mee", 4, ["attach", "meeting"]).map((a) => a.id),
  ).toEqual(["meeting"]);
  expect(slashCommands("/meeting", 8, ["attach"])).toEqual([]);
  for (const value of [
    "https://example.com/",
    "A /meeting",
    "```/meeting",
    "/unknown",
    "/meeting/path",
  ]) {
    expect(slashCommands(value, value.length, ["meeting", "attach"])).toEqual(
      [],
    );
  }
  expect(slashCommands("/meeting original draft", 8, ["meeting"])).toHaveLength(
    1,
  );
  expect(slashCommands("/meeting/path", 8, ["meeting"])).toEqual([]);
});

test("staged source identity and revision participate in retry keys", () => {
  const first = sendAttemptKey("Review", [], null, { id: "a", revision: 1 });
  expect(first).toBe(
    sendAttemptKey("Review", [], null, { id: "a", revision: 1 }),
  );
  expect(first).not.toBe(
    sendAttemptKey("Review", [], null, { id: "b", revision: 1 }),
  );
  expect(first).not.toBe(
    sendAttemptKey("Review", [], null, { id: "a", revision: 2 }),
  );
  expect(sendAttemptKey("Review", [], null, null)).toBe("Review");
});
