import { describe, expect, test } from "bun:test";
import {
  calendarEventsToScheduleBlocks,
  parseBlocks,
  reconcileScheduleBlocks,
  scheduleEditorSeed,
} from "../src/Today";
import type { CalEvent } from "../src/api";

function calendarEvent(overrides: Partial<CalEvent> = {}): CalEvent {
  return {
    id: "event-1",
    task: "Calendar planning",
    start: "09:00",
    end: "10:00",
    all_day: false,
    calendar: "Work",
    calendar_id: "work-calendar",
    account: "person@example.com",
    meet_link: null,
    html_link: null,
    ...overrides,
  };
}

describe("schedule builder sources", () => {
  test("does not seed a schedule from a task-only daily entry", () => {
    const taskOnlyEntry = {
      raw_text: "[ ] Send the proposal\n[ ] Review the budget",
      data: {
        blocks: [],
        todos: [
          { id: "task-1", text: "Send the proposal", completed: false },
          { id: "task-2", text: "Review the budget", completed: false },
        ],
      },
    };

    // The raw text intentionally is not an input to scheduleEditorSeed. Only
    // structured schedule blocks can reopen an existing schedule.
    expect(scheduleEditorSeed(parseBlocks(taskOnlyEntry.data))).toBe("");
  });

  test("reopens timed schedule blocks but excludes untimed task-like rows", () => {
    expect(
      scheduleEditorSeed(
        parseBlocks({
          blocks: [
            { task: "Team meeting", start: "09:00", end: "10:00" },
            { task: "Send the proposal" },
          ],
        }),
      ),
    ).toBe("9:00 AM–10:00 AM Team meeting");
  });

  test("builds only from timed events returned by the calendar API", () => {
    const events = [
      calendarEvent(),
      calendarEvent({ id: "all-day", task: "Company holiday", start: null, end: null, all_day: true }),
      calendarEvent({ id: "bad-time", task: "Malformed event", start: "later" }),
      calendarEvent({ id: "blank", task: "   " }),
    ];

    expect(calendarEventsToScheduleBlocks(events)).toEqual([
      { task: "Calendar planning", start: "09:00", end: "10:00" },
    ]);
  });

  test("does not combine open tasks with the calendar schedule seed", () => {
    const openTasks = [
      { id: "task-1", text: "Send the proposal", completed: false },
      { id: "task-2", text: "Review the budget", completed: false },
    ];
    const blocks = calendarEventsToScheduleBlocks([calendarEvent({ task: "Customer call" })]);

    expect(blocks.map((block) => block.task)).toEqual(["Customer call"]);
    expect(blocks.some((block) => openTasks.some((task) => task.text === block.task))).toBe(false);
  });

  test("reconciles legacy Eastern clock strings from a unique live calendar event", () => {
    const blocks = [{ task: "Daily Stand Up", start: "11:00", end: "11:15" }];
    const events = [calendarEvent({ task: "Daily Stand Up", start: "08:00", end: "08:15" })];

    expect(reconcileScheduleBlocks(blocks, events)).toEqual([
      { task: "Daily Stand Up", start: "08:00", end: "08:15" },
    ]);
  });

  test("keeps duplicate calendar titles ambiguous", () => {
    const blocks = [{ task: "Focus time", start: "11:00", end: "12:00" }];
    const events = [
      calendarEvent({ id: "one", task: "Focus time", start: "08:00", end: "09:00" }),
      calendarEvent({ id: "two", task: "Focus time", start: "13:00", end: "14:00" }),
    ];

    expect(reconcileScheduleBlocks(blocks, events)).toEqual(blocks);
  });
});
