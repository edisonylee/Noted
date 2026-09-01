import { describe, expect, test } from "bun:test";
import {
  persistDailyTaskDocument,
  type DailyTaskPersistence,
} from "../src/dailyTaskPersistence";
import type { StructuredDocument } from "../src/editor/document";

const document: StructuredDocument = {
  type: "doc",
  content: [
    {
      type: "taskList",
      content: [
        {
          type: "taskItem",
          attrs: { checked: false },
          content: [
            {
              type: "paragraph",
              content: [{ type: "text", text: "Send the proposal" }],
            },
          ],
        },
      ],
    },
  ],
};

function persistenceLog(calls: string[]): DailyTaskPersistence {
  return {
    updateEntry: async (entryId, data) => {
      calls.push(`update:${entryId}:${data.todos[0]?.text}`);
    },
    createEntry: async (args) => {
      calls.push(`create:${args.event_date}:${args.raw_text}`);
    },
    refreshEntries: async () => {
      calls.push("refresh");
    },
  };
}

describe("daily task persistence", () => {
  test("refreshes the schedule cache after updating an existing day", async () => {
    const calls: string[] = [];

    await persistDailyTaskDocument({
      entryId: 42,
      targetDate: "2026-09-01",
      document,
      blocks: [],
      persistence: persistenceLog(calls),
    });

    expect(calls).toEqual(["update:42:Send the proposal", "refresh"]);
  });

  test("refreshes the schedule cache after creating a new day", async () => {
    const calls: string[] = [];

    await persistDailyTaskDocument({
      entryId: null,
      targetDate: "2026-09-02",
      document,
      blocks: [{ task: "Planning", start: "09:00", end: "10:00" }],
      persistence: persistenceLog(calls),
    });

    expect(calls).toEqual([
      "create:2026-09-02:- [ ] Send the proposal",
      "refresh",
    ]);
  });

  test("does not refresh stale data when the database write fails", async () => {
    const calls: string[] = [];
    const persistence = persistenceLog(calls);
    persistence.updateEntry = async () => {
      calls.push("update");
      throw new Error("write failed");
    };

    await expect(
      persistDailyTaskDocument({
        entryId: 42,
        targetDate: "2026-09-01",
        document,
        blocks: [],
        persistence,
      }),
    ).rejects.toThrow("write failed");
    expect(calls).toEqual(["update"]);
  });
});
