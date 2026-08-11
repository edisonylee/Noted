import { describe, expect, test } from "bun:test";
import {
  assignOverlapLanes,
  buildScheduleGrid,
  computeEventGeometry,
  isCurrentInterval,
  resolveEventEnd,
  scheduleGridBounds,
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
  test("renders the full duration of a long event", () => {
    const grid = buildScheduleGrid(
      [
        { task: "Long event", start: "12:00", end: "16:45" },
        { task: "Overlap", start: "13:00", end: "13:45" },
      ],
      { pixelsPerHour: 40, minHeightPx: 34, gapPx: 2 },
    );
    expect(grid.items[0]).toMatchObject({ topPx: 160, heightPx: 188, lane: 0, laneCount: 2 });
    expect(grid.items[1]).toMatchObject({ lane: 1, laneCount: 2 });
  });

  test("expands bounds for early, late, and overnight events", () => {
    expect(scheduleGridBounds([{ index: 0, start: 450, end: 525 }])).toEqual({ start: 420, end: 1200 });
    expect(scheduleGridBounds([{ index: 0, start: 1275, end: 1350 }])).toEqual({ start: 480, end: 1380 });
    expect(scheduleGridBounds([{ index: 0, start: 1380, end: 1500 }])).toEqual({ start: 480, end: 1500 });
  });

  test("marks every active overlap as current with an exclusive end", () => {
    const long = { index: 0, start: 720, end: 1005 };
    const overlap = { index: 1, start: 780, end: 825 };
    expect(isCurrentInterval(long, 810)).toBe(true);
    expect(isCurrentInterval(overlap, 810)).toBe(true);
    expect(isCurrentInterval(overlap, 825)).toBe(false);
  });
});
