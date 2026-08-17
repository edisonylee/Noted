export type ScheduleClockBlock = {
  start?: string;
  end?: string;
  duration_min?: number;
};

export type ScheduleInterval = {
  index: number;
  start: number;
  end: number;
};

export type LanedScheduleInterval = ScheduleInterval & {
  group: number;
  lane: number;
  laneCount: number;
};

export type ScheduleGeometry = LanedScheduleInterval & {
  topPx: number;
  heightPx: number;
  leftFraction: number;
  widthFraction: number;
};

export type ScheduleGridItem<T> = ScheduleGeometry & {
  block: T;
  durationMinutes: number;
};

export type ScheduleGridLayout<T> = {
  start: number;
  end: number;
  heightPx: number;
  hourMarks: number[];
  items: ScheduleGridItem<T>[];
};

export type AdaptiveScheduleBand = {
  start: number;
  end: number;
  topPx: number;
  heightPx: number;
  kind: "focus" | "gap";
  compressed: boolean;
};

export type AdaptiveScheduleSegment = AdaptiveScheduleBand;

export type AdaptiveScheduleMark = {
  minute: number;
  topPx: number;
  major: boolean;
};

export type AdaptiveScheduleScale = {
  start: number;
  end: number;
  heightPx: number;
  bands: AdaptiveScheduleBand[];
  segments: AdaptiveScheduleSegment[];
  marks: AdaptiveScheduleMark[];
  minuteToY: (minute: number) => number;
  yToMinute: (offsetPx: number) => number;
};

export function scheduleTimeToMinutes(value?: string): number | null {
  if (!value) return null;
  const match = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (hour > 23 || minute > 59) return null;
  return hour * 60 + minute;
}

export function resolveEventEnd(
  start: number,
  block: Pick<ScheduleClockBlock, "end" | "duration_min">,
  fallbackMinutes = 60,
): number {
  const endClock = scheduleTimeToMinutes(block.end);
  if (endClock != null) {
    const dayStart = Math.floor(start / 1440) * 1440;
    let explicitEnd = dayStart + endClock;
    if (explicitEnd < start) explicitEnd += 1440;
    if (explicitEnd > start) return explicitEnd;
  }

  if (Number.isFinite(block.duration_min) && block.duration_min! > 0) {
    return start + block.duration_min!;
  }
  return start + fallbackMinutes;
}

export function scheduleIntervals<T extends ScheduleClockBlock>(blocks: readonly T[]): Array<ScheduleInterval & { block: T }> {
  const intervals: Array<ScheduleInterval & { block: T }> = [];
  let carry = 0;
  let previousStart = -1;

  blocks.forEach((block, index) => {
    const clock = scheduleTimeToMinutes(block.start);
    if (clock == null) return;
    if (carry + clock < previousStart) carry += 1440;
    const start = carry + clock;
    previousStart = start;
    intervals.push({ index, block, start, end: resolveEventEnd(start, block) });
  });

  return intervals;
}

export function assignOverlapLanes(events: readonly ScheduleInterval[]): LanedScheduleInterval[] {
  const sorted = events
    .map((event) => ({ ...event, collisionEnd: event.end }))
    .sort((a, b) => a.start - b.start || b.end - a.end || a.index - b.index);
  const output: LanedScheduleInterval[] = [];
  let group = 0;
  let cursor = 0;

  while (cursor < sorted.length) {
    const members = [sorted[cursor]];
    let groupEnd = sorted[cursor].collisionEnd;
    cursor += 1;
    while (cursor < sorted.length && sorted[cursor].start < groupEnd) {
      members.push(sorted[cursor]);
      groupEnd = Math.max(groupEnd, sorted[cursor].collisionEnd);
      cursor += 1;
    }

    const laneEnds: number[] = [];
    const assigned = members.map((event) => {
      let lane = laneEnds.findIndex((end) => end <= event.start);
      if (lane === -1) lane = laneEnds.length;
      laneEnds[lane] = event.collisionEnd;
      return { index: event.index, start: event.start, end: event.end, group, lane };
    });
    const laneCount = laneEnds.length;
    output.push(...assigned.map((event) => ({ ...event, laneCount })));
    group += 1;
  }

  return output;
}

export function computeEventGeometry(
  events: readonly ScheduleInterval[],
  {
    gridStart,
    pixelsPerHour,
    minHeightPx,
    gapPx,
  }: {
    gridStart: number;
    pixelsPerHour: number;
    minHeightPx: number;
    gapPx: number;
  },
): ScheduleGeometry[] {
  const pixelsPerMinute = pixelsPerHour / 60;
  return assignOverlapLanes(events).map((event) => ({
    ...event,
    topPx: (event.start - gridStart) * pixelsPerMinute,
    heightPx: Math.max(minHeightPx, (event.end - event.start) * pixelsPerMinute - gapPx),
    leftFraction: event.lane / event.laneCount,
    widthFraction: 1 / event.laneCount,
  }));
}

export function scheduleGridBounds(
  events: readonly ScheduleInterval[],
  {
    defaultStart = 6 * 60,
    defaultEnd = 24 * 60,
    tick = 60,
  }: { defaultStart?: number; defaultEnd?: number; tick?: number } = {},
): { start: number; end: number } {
  if (!events.length) return { start: defaultStart, end: defaultEnd };
  const earliest = Math.min(...events.map((event) => event.start));
  const latest = Math.max(...events.map((event) => event.end));
  return {
    start: Math.min(defaultStart, Math.floor(earliest / tick) * tick),
    end: Math.max(defaultEnd, Math.ceil(latest / tick) * tick),
  };
}

export function snapScheduleMinute(value: number, stepMinutes = 15): number {
  const step = Number.isFinite(stepMinutes) && stepMinutes > 0 ? stepMinutes : 15;
  return Math.round(value / step) * step;
}

function alignScheduleMinute(value: number, step: number, direction: "down" | "up"): number {
  return (direction === "down" ? Math.floor(value / step) : Math.ceil(value / step)) * step;
}

/**
 * Builds a piecewise time scale for a day view. Busy clusters retain a calm,
 * readable rhythm while long empty stretches collapse into clearly labelled
 * gaps. Time still maps continuously in both directions, so direct creation
 * and resizing remain accurate inside compressed space.
 */
export function buildAdaptiveScheduleScale(
  events: readonly LanedScheduleInterval[],
  {
    gridStart,
    gridEnd,
    focusPixelsPerHour = 72,
    emptyPixelsPerHour = 44,
    minEventHeightPx = 40,
    eventGapPx = 4,
    compressedGapMinMinutes = 90,
  }: {
    gridStart: number;
    gridEnd: number;
    focusPixelsPerHour?: number;
    emptyPixelsPerHour?: number;
    minEventHeightPx?: number;
    eventGapPx?: number;
    compressedGapMinMinutes?: number;
  },
): AdaptiveScheduleScale {
  const start = Math.min(gridStart, gridEnd);
  const end = Math.max(gridStart, gridEnd);
  const visibleEvents = events.filter((event) => event.end > start && event.start < end);
  const focusWindows = visibleEvents
    .map((event) => {
      const eventStart = Math.max(start, event.start);
      const eventEnd = Math.min(end, event.end);
      const isShort = eventEnd - eventStart < 60;
      return {
        start: Math.max(
          start,
          alignScheduleMinute(eventStart, 30, "down") - (isShort ? 30 : 0),
        ),
        end: Math.min(
          end,
          alignScheduleMinute(eventEnd, 30, "up"),
        ),
      };
    })
    .sort((a, b) => a.start - b.start || a.end - b.end)
    .reduce<Array<{ start: number; end: number }>>((merged, window) => {
      const previous = merged[merged.length - 1];
      if (previous && window.start <= previous.end) {
        previous.end = Math.max(previous.end, window.end);
      } else {
        merged.push({ ...window });
      }
      return merged;
    }, []);

  const bandDefinitions: Array<Pick<AdaptiveScheduleBand, "start" | "end" | "kind" | "compressed">> = [];
  if (!focusWindows.length) {
    bandDefinitions.push({ start, end, kind: "focus", compressed: false });
  } else {
    let cursor = start;
    focusWindows.forEach((window) => {
      if (window.start > cursor) {
        const duration = window.start - cursor;
        bandDefinitions.push({
          start: cursor,
          end: window.start,
          kind: "gap",
          compressed: duration >= compressedGapMinMinutes,
        });
      }
      bandDefinitions.push({ ...window, kind: "focus", compressed: false });
      cursor = window.end;
    });
    if (cursor < end) {
      const duration = end - cursor;
      bandDefinitions.push({
        start: cursor,
        end,
        kind: "gap",
        compressed: duration >= compressedGapMinMinutes,
      });
    }
  }

  const segments: AdaptiveScheduleSegment[] = [];
  const bands: AdaptiveScheduleBand[] = [];
  let topPx = 0;

  bandDefinitions.forEach((band) => {
    const bandTop = topPx;
    const duration = band.end - band.start;
    if (band.kind === "gap") {
      const heightPx = band.compressed
        ? Math.min(74, Math.max(58, 52 + (duration / 60) * 4))
        : (duration / 60) * emptyPixelsPerHour;
      segments.push({ ...band, topPx, heightPx });
      topPx += heightPx;
    } else {
      const starts = visibleEvents
        .filter((event) => event.start >= band.start && event.start < band.end)
        .map((event) => event.start);
      const anchors = Array.from(new Set([band.start, ...starts, band.end])).sort((a, b) => a - b);
      for (let index = 0; index < anchors.length - 1; index += 1) {
        const segmentStart = anchors[index];
        const segmentEnd = anchors[index + 1];
        const eventsAtStart = visibleEvents.filter((event) => event.start === segmentStart);
        const eventsAtEnd = visibleEvents.filter((event) => event.start === segmentEnd);
        const sharesLaneWithNext = eventsAtStart.some((current) =>
          eventsAtEnd.some((next) => current.lane === next.lane),
        );
        const isLastAnchor = segmentEnd === band.end;
        const clearance =
          eventsAtStart.length && (sharesLaneWithNext || isLastAnchor)
            ? minEventHeightPx + eventGapPx
            : 0;
        const heightPx = Math.max(
          ((segmentEnd - segmentStart) / 60) * focusPixelsPerHour,
          clearance,
        );
        segments.push({
          start: segmentStart,
          end: segmentEnd,
          topPx,
          heightPx,
          kind: "focus",
          compressed: false,
        });
        topPx += heightPx;
      }
    }
    bands.push({ ...band, topPx: bandTop, heightPx: topPx - bandTop });
  });

  const minuteToY = (minute: number) => {
    if (minute <= start || !segments.length) return 0;
    if (minute >= end) return topPx;
    const segment = segments.find((candidate) => minute >= candidate.start && minute <= candidate.end);
    if (!segment || segment.end === segment.start) return 0;
    const progress = (minute - segment.start) / (segment.end - segment.start);
    return segment.topPx + progress * segment.heightPx;
  };

  const yToMinute = (offsetPx: number) => {
    if (offsetPx <= 0 || !segments.length) return start;
    if (offsetPx >= topPx) return end;
    const segment = segments.find(
      (candidate) => offsetPx >= candidate.topPx && offsetPx <= candidate.topPx + candidate.heightPx,
    );
    if (!segment || segment.heightPx === 0) return start;
    const progress = (offsetPx - segment.topPx) / segment.heightPx;
    return segment.start + progress * (segment.end - segment.start);
  };

  const markMinutes = new Set<number>();
  bands.forEach((band) => {
    if (band.compressed) return;
    for (
      let minute = alignScheduleMinute(band.start, 30, "up");
      minute <= band.end;
      minute += 30
    ) {
      markMinutes.add(minute);
    }
  });
  const marks = Array.from(markMinutes)
    .sort((a, b) => a - b)
    .map((minute) => ({ minute, topPx: minuteToY(minute), major: minute % 60 === 0 }));

  return { start, end, heightPx: topPx, bands, segments, marks, minuteToY, yToMinute };
}

export function scheduleMinuteFromGridOffset(
  offsetPx: number,
  {
    gridStart,
    pixelsPerHour,
    stepMinutes = 15,
  }: {
    gridStart: number;
    pixelsPerHour: number;
    stepMinutes?: number;
  },
): number {
  if (!Number.isFinite(pixelsPerHour) || pixelsPerHour <= 0) return gridStart;
  const minute = gridStart + (offsetPx / pixelsPerHour) * 60;
  return snapScheduleMinute(minute, stepMinutes);
}

export function scheduleEndFromResizeDelta(
  start: number,
  initialEnd: number,
  deltaPx: number,
  {
    pixelsPerHour,
    stepMinutes = 15,
    maxEnd = Number.POSITIVE_INFINITY,
  }: {
    pixelsPerHour: number;
    stepMinutes?: number;
    maxEnd?: number;
  },
): number {
  const step = Number.isFinite(stepMinutes) && stepMinutes > 0 ? stepMinutes : 15;
  if (!Number.isFinite(pixelsPerHour) || pixelsPerHour <= 0) {
    return Math.min(maxEnd, Math.max(start + step, initialEnd));
  }
  const deltaMinutes = Math.round(((deltaPx / pixelsPerHour) * 60) / step) * step;
  return Math.min(maxEnd, Math.max(start + step, initialEnd + deltaMinutes));
}

export function scheduleStartFromResizeDelta(
  initialStart: number,
  end: number,
  deltaPx: number,
  {
    pixelsPerHour,
    stepMinutes = 15,
    minStart = Number.NEGATIVE_INFINITY,
  }: {
    pixelsPerHour: number;
    stepMinutes?: number;
    minStart?: number;
  },
): number {
  const step = Number.isFinite(stepMinutes) && stepMinutes > 0 ? stepMinutes : 15;
  if (!Number.isFinite(pixelsPerHour) || pixelsPerHour <= 0) {
    return Math.max(minStart, Math.min(end - step, initialStart));
  }
  const deltaMinutes = Math.round(((deltaPx / pixelsPerHour) * 60) / step) * step;
  return Math.max(minStart, Math.min(end - step, initialStart + deltaMinutes));
}

export function buildScheduleGrid<T extends ScheduleClockBlock>(
  blocks: readonly T[],
  {
    pixelsPerHour = 44,
    minHeightPx = 40,
    gapPx = 3,
    defaultStart = 6 * 60,
    defaultEnd = 24 * 60,
  }: {
    pixelsPerHour?: number;
    minHeightPx?: number;
    gapPx?: number;
    defaultStart?: number;
    defaultEnd?: number;
  } = {},
): ScheduleGridLayout<T> {
  const intervals = scheduleIntervals(blocks);
  const bounds = scheduleGridBounds(intervals, { defaultStart, defaultEnd });
  const geometry = computeEventGeometry(intervals, {
    gridStart: bounds.start,
    pixelsPerHour,
    minHeightPx,
    gapPx,
  });
  const byIndex = new Map(intervals.map((event) => [event.index, event]));
  const items = geometry.map((event) => {
    const source = byIndex.get(event.index)!;
    return {
      ...event,
      block: source.block,
      durationMinutes: event.end - event.start,
    };
  });
  const hourMarks: number[] = [];
  for (let minute = bounds.start; minute <= bounds.end; minute += 60) hourMarks.push(minute);
  return {
    start: bounds.start,
    end: bounds.end,
    heightPx: ((bounds.end - bounds.start) / 60) * pixelsPerHour,
    hourMarks,
    items,
  };
}

export function isCurrentInterval(event: ScheduleInterval, now: number): boolean {
  return now >= event.start && now < event.end;
}
