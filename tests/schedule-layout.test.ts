import { describe, expect, test } from "bun:test";
import {
  assignOverlapLanes,
  buildFixedScheduleScale,
  buildScheduleGrid,
  computeEventGeometry,
  isCurrentInterval,
  resolveEventEnd,
  scheduleEndFromResizeDelta,
  scheduleEventHeightPx,
  scheduleGridBounds,
  scheduleMinuteFromGridOffset,
  scheduleRangeFromDrag,
  scheduleRangeFromMove,
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

  test("keeps close but non-overlapping events full width", () => {
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
    expect(geometry.map((event) => ({ lane: event.lane, laneCount: event.laneCount }))).toEqual([
      { lane: 0, laneCount: 1 },
      { lane: 0, laneCount: 1 },
    ]);
    expect(geometry[0].heightPx).toBe(34);
  });
});

describe("fixed schedule scale", () => {
  const blocks = [
    { task: "Stand-up", start: "08:00", end: "08:15" },
    { task: "One-on-one", start: "09:00", end: "09:15" },
    { task: "Check-in", start: "09:45", end: "10:00" },
    { task: "Review", start: "10:00", end: "10:15" },
    { task: "Team event", start: "14:30", end: "17:30" },
    { task: "Gym", start: "17:00", end: "18:30" },
  ];

  test("keeps the same half-hour rhythm through busy and empty time", () => {
    const grid = buildScheduleGrid(blocks);
    const scale = buildFixedScheduleScale({
      gridStart: grid.start,
      gridEnd: grid.end,
      pixelsPerHour: 72,
      markMinutes: 30,
    });

    expect(scale.minuteToY(8 * 60 + 30) - scale.minuteToY(8 * 60)).toBe(36);
    expect(scale.minuteToY(13 * 60 + 30) - scale.minuteToY(13 * 60)).toBe(36);
    expect(scale.heightPx).toBe(18 * 72);
    expect(scale.marks.every((mark, index, marks) =>
      index === 0 || mark.minute - marks[index - 1].minute === 30
    )).toBe(true);

    for (const minute of [6 * 60, 8 * 60, 9 * 60 + 45, 13 * 60, 18 * 60 + 30, 24 * 60]) {
      expect(scale.yToMinute(scale.minuteToY(minute))).toBeCloseTo(minute, 6);
    }
  });

  test("keeps short event cards inside their real time slot", () => {
    const scale = buildFixedScheduleScale({
      gridStart: 6 * 60,
      gridEnd: 24 * 60,
      pixelsPerHour: 72,
    });

    expect(scheduleEventHeightPx(scale, 9 * 60 + 45, 10 * 60, 2)).toBe(16);
    expect(scheduleEventHeightPx(scale, 10 * 60, 10 * 60 + 15, 2)).toBe(16);
    expect(scheduleEventHeightPx(scale, 14 * 60 + 30, 17 * 60 + 30, 2)).toBe(214);
  });

  test("uses columns only for real overlaps", () => {
    const grid = buildScheduleGrid(blocks);
    expect(grid.items.slice(0, 4).every((item) => item.laneCount === 1)).toBe(true);
    expect(grid.items.slice(4).map((item) => item.laneCount)).toEqual([2, 2]);
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
  test("creates a default block on click and an exact range on drag", () => {
    expect(
      scheduleRangeFromDrag(9 * 60, 9 * 60, {
        minStart: 6 * 60,
        maxEnd: 24 * 60,
        dragged: false,
      }),
    ).toEqual({ start: 9 * 60, end: 10 * 60 });
    expect(
      scheduleRangeFromDrag(9 * 60, 10 * 60 + 30, {
        minStart: 6 * 60,
        maxEnd: 24 * 60,
        dragged: true,
      }),
    ).toEqual({ start: 9 * 60, end: 10 * 60 + 30 });
    expect(
      scheduleRangeFromDrag(10 * 60, 8 * 60 + 30, {
        minStart: 6 * 60,
        maxEnd: 24 * 60,
        dragged: true,
      }),
    ).toEqual({ start: 8 * 60 + 30, end: 10 * 60 });
  });

  test("moves a block in snapped steps without crossing the day edges", () => {
    expect(
      scheduleRangeFromMove(9 * 60, 10 * 60, 22, {
        minStart: 6 * 60,
        maxEnd: 24 * 60,
      }),
    ).toEqual({ start: 9 * 60 + 15, end: 10 * 60 + 15 });
    expect(
      scheduleRangeFromMove(6 * 60, 7 * 60, -90, {
        minStart: 6 * 60,
        maxEnd: 24 * 60,
      }),
    ).toEqual({ start: 6 * 60, end: 7 * 60 });
    expect(
      scheduleRangeFromMove(23 * 60, 24 * 60, 90, {
        minStart: 6 * 60,
        maxEnd: 24 * 60,
      }),
    ).toEqual({ start: 23 * 60, end: 24 * 60 });
  });

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
