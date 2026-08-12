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

export function assignOverlapLanes(
  events: readonly ScheduleInterval[],
  minimumSpanMinutes = 0,
): LanedScheduleInterval[] {
  const sorted = events
    .map((event) => ({ ...event, collisionEnd: Math.max(event.end, event.start + minimumSpanMinutes) }))
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
  const minimumSpanMinutes = (minHeightPx + gapPx) / pixelsPerMinute;
  return assignOverlapLanes(events, minimumSpanMinutes).map((event) => ({
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
