import { describe, expect, test } from "bun:test";
import {
  assignOverlapLanes,
  buildScheduleGrid,
  computeEventGeometry,
  isCurrentInterval,
  resolveEventEnd,
  scheduleEndFromResizeDelta,
  scheduleGridBounds,
  scheduleMinuteFromGridOffset,
  scheduleStartFromResizeDelta,
  type ScheduleInterval,
} from "../src/scheduleLayout";

describe("schedule event duration", () => {
  test("prefers an explicit end and handles midnight", () => {
    expect(resolveEventEnd(720, { end: "16:45" })).toBe(1005);
    expect(resolveEventEnd(1380, { end: "01:00" })).toBe(1500);
  });

  test("falls back to duration and then one hour", () => {
    expect(resolveEventEnd(720, { duration_min: 45 })).toBe(765);
    expect(resolveEventEnd(720, {})).toBe(780);
  });
});

describe("schedule overlap lanes", () => {
  test("places a nested event beside a long event", () => {
    const lanes = assignOverlapLanes([
      { index: 0, start: 720, end: 1005 },
      { index: 1, start: 780, end: 825 },
    ]);
    expect(lanes.map(({ lane, laneCount }) => ({ lane, laneCount }))).toEqual([
      { lane: 0, laneCount: 2 },
      { lane: 1, laneCount: 2 },
    ]);
  });

  test("reuses lanes when events touch or an overlap ends", () => {
    const lanes = assignOverlapLanes([
      { index: 0, start: 720, end: 1005 },
      { index: 1, start: 780, end: 825 },
      { index: 2, start: 825, end: 870 },
    ]);
    expect(lanes.map((event) => event.lane)).toEqual([0, 1, 1]);
  });

  test("accounts for the painted minimum height", () => {
    const events: ScheduleInterval[] = [
      { index: 0, start: 720, end: 735 },
      { index: 1, start: 750, end: 765 },
    ];
    const geometry = computeEventGeometry(events, {
      gridStart: 480,
      pixelsPerHour: 40,
      minHeightPx: 34,
      gapPx: 2,
    });
    expect(geometry.map((event) => event.lane)).toEqual([0, 1]);
    expect(geometry[0].heightPx).toBe(34);
  });
});

describe("schedule grid geometry", () => {
  test("keeps an empty day as a full interactive canvas", () => {
    const grid = buildScheduleGrid([]);
    expect(grid.start).toBe(6 * 60);
    expect(grid.end).toBe(24 * 60);
    expect(grid.hourMarks[0]).toBe(6 * 60);
    expect(grid.hourMarks.at(-1)).toBe(24 * 60);
    expect(grid.items).toEqual([]);
  });

  test("renders the full duration of a long event", () => {
    const grid = buildScheduleGrid(
      [
        { task: "Long event", start: "12:00", end: "16:45" },
        { task: "Overlap", start: "13:00", end: "13:45" },
      ],
      { pixelsPerHour: 40, minHeightPx: 34, gapPx: 2 },
    );
    expect(grid.items[0]).toMatchObject({ topPx: 240, heightPx: 188, lane: 0, laneCount: 2 });
    expect(grid.items[1]).toMatchObject({ lane: 1, laneCount: 2 });
  });

  test("expands bounds for early, late, and overnight events", () => {
    expect(scheduleGridBounds([{ index: 0, start: 450, end: 525 }])).toEqual({ start: 360, end: 1440 });
    expect(scheduleGridBounds([{ index: 0, start: 1275, end: 1350 }])).toEqual({ start: 360, end: 1440 });
    expect(scheduleGridBounds([{ index: 0, start: 1380, end: 1500 }])).toEqual({ start: 360, end: 1500 });
  });

  test("marks every active overlap as current with an exclusive end", () => {
    const long = { index: 0, start: 720, end: 1005 };
    const overlap = { index: 1, start: 780, end: 825 };
    expect(isCurrentInterval(long, 810)).toBe(true);
    expect(isCurrentInterval(overlap, 810)).toBe(true);
    expect(isCurrentInterval(overlap, 825)).toBe(false);
  });
});

describe("direct schedule manipulation", () => {
  test("maps a grid click to the nearest 15-minute start", () => {
    expect(
      scheduleMinuteFromGridOffset(132, {
        gridStart: 8 * 60,
        pixelsPerHour: 44,
      }),
    ).toBe(11 * 60);
    expect(
      scheduleMinuteFromGridOffset(143, {
        gridStart: 8 * 60,
        pixelsPerHour: 44,
      }),
    ).toBe(11 * 60 + 15);
  });

  test("resizes in 15-minute steps and keeps a positive duration", () => {
    expect(
      scheduleEndFromResizeDelta(12 * 60, 13 * 60, 66, {
        pixelsPerHour: 44,
      }),
    ).toBe(14 * 60 + 30);
    expect(
      scheduleEndFromResizeDelta(12 * 60, 13 * 60, -200, {
        pixelsPerHour: 44,
      }),
    ).toBe(12 * 60 + 15);

    expect(
      scheduleStartFromResizeDelta(12 * 60, 13 * 60, -66, {
        pixelsPerHour: 44,
      }),
    ).toBe(10 * 60 + 30);
    expect(
      scheduleStartFromResizeDelta(12 * 60, 13 * 60, 200, {
        pixelsPerHour: 44,
      }),
    ).toBe(12 * 60 + 45);
  });

  test("does not resize past the visible grid boundary", () => {
    expect(
      scheduleEndFromResizeDelta(19 * 60, 20 * 60, 100, {
        pixelsPerHour: 44,
        maxEnd: 20 * 60,
      }),
    ).toBe(20 * 60);
    expect(
      scheduleStartFromResizeDelta(7 * 60, 8 * 60, -100, {
        pixelsPerHour: 44,
        minStart: 6 * 60,
      }),
    ).toBe(6 * 60);
  });
});
