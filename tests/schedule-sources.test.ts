import { describe, expect, test } from "bun:test";
import {
  calendarEventsToScheduleBlocks,
  mergeScheduleWithCalendar,
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
    color: "#7986cb",
    account: "person@example.com",
    meet_link: null,
    html_link: null,
    ...overrides,
  };
}

describe("automatic schedule sources", () => {
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

  test("automatically turns timed calendar events into the day's schedule", () => {
    expect(
      mergeScheduleWithCalendar([], [
        calendarEvent({ id: "later", task: "Customer call", start: "13:00", end: "13:30" }),
        calendarEvent({ id: "earlier", task: "Daily stand-up", start: "08:30", end: "08:45" }),
      ]),
    ).toEqual([
      { task: "Daily stand-up", start: "08:30", end: "08:45" },
      { task: "Customer call", start: "13:00", end: "13:30" },
    ]);
  });

  test("preserves manual schedule items while adding only missing calendar events", () => {
    const blocks = [
      { task: "Write launch brief", start: "10:00", end: "11:00" },
      { task: "Calendar planning", start: "09:00", end: "10:00" },
    ];

    expect(
      mergeScheduleWithCalendar(blocks, [
        calendarEvent(),
        calendarEvent({ id: "review", task: "Launch review", start: "14:00", end: "15:00" }),
      ]),
    ).toEqual([
      { task: "Calendar planning", start: "09:00", end: "10:00" },
      { task: "Write launch brief", start: "10:00", end: "11:00" },
      { task: "Launch review", start: "14:00", end: "15:00" },
    ]);
  });

  test("reconciles a unique moved event without creating a duplicate", () => {
    expect(
      mergeScheduleWithCalendar(
        [{ task: "Calendar planning", start: "12:00", end: "13:00" }],
        [calendarEvent()],
      ),
    ).toEqual([{ task: "Calendar planning", start: "09:00", end: "10:00" }]);
  });

  test("does not reorder a hand-authored schedule when Calendar has nothing to add", () => {
    const blocks = [
      { task: "Late writing", start: "22:00", end: "23:00" },
      { task: "Sleep", start: "00:00", end: "07:00" },
    ];

    expect(mergeScheduleWithCalendar(blocks, [])).toEqual(blocks);
  });

  test("keeps distinct same-title calendar events", () => {
    expect(
      mergeScheduleWithCalendar([], [
        calendarEvent({ id: "morning", task: "Focus time", start: "09:00", end: "10:00" }),
        calendarEvent({ id: "afternoon", task: "Focus time", start: "14:00", end: "15:00" }),
      ]),
    ).toEqual([
      { task: "Focus time", start: "09:00", end: "10:00" },
      { task: "Focus time", start: "14:00", end: "15:00" },
    ]);
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
