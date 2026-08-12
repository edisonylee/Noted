// A single, quiet handoff from today's agenda to the next external calendar
// event. The daily schedule itself already carries today's local plan, so this
// row deliberately excludes noted's pushed calendar to avoid echoing it.

import { useCallback, useEffect, useMemo, useState } from "react";
import { CalendarDays, ChevronRight } from "lucide-react";
import { api, type RangeEvent } from "./api";
import { easternDay, easternMinutes } from "./day";

function fmtClock(min: number | null): string {
  if (min == null) return "All day";
  const h = Math.floor((min % 1440) / 60);
  const m = min % 60;
  const ampm = h >= 12 ? "PM" : "AM";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return `${h12}:${String(m).padStart(2, "0")} ${ampm}`;
}

function dayLabel(date: string, today: string): string {
  if (date === today) return "Today";
  const tomorrow = easternDay(new Date(Date.now() + 86_400_000));
  if (date === tomorrow) return "Tomorrow";
  return new Date(`${date}T12:00:00`).toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
}

export function ComingUp({ onOpenEvent }: { onOpenEvent: (ev: RangeEvent) => void }) {
  const [events, setEvents] = useState<RangeEvent[] | null>(null);
  const [nowMin, setNowMin] = useState(easternMinutes());

  const load = useCallback(async () => {
    const today = easternDay();
    const tomorrow = easternDay(new Date(Date.now() + 86_400_000));
    try {
      setEvents(await api.gcalEventsRange(today, tomorrow));
    } catch {
      setEvents([]);
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      setNowMin(easternMinutes());
      void load();
    }, 300_000);
    return () => window.clearInterval(timer);
  }, [load]);

  const next = useMemo(() => {
    if (!events) return null;
    const today = easternDay();
    const relevant = events
      .filter((event) => !event.declined)
      .filter((event) => event.calendar.toLowerCase() !== "noted")
      .filter((event) => {
        if (event.date !== today) return event.date > today;
        if (event.all_day || event.start_min == null) return false;
        return event.start_min > nowMin;
      })
      .sort((a, b) => {
        if (a.date !== b.date) return a.date < b.date ? -1 : 1;
        return (a.start_min ?? -1) - (b.start_min ?? -1);
      });

    const timed = relevant.find((event) => !event.all_day && event.start_min != null);
    return timed ?? relevant[0] ?? null;
  }, [events, nowMin]);

  if (!next) return null;

  const today = easternDay();
  const when = `${dayLabel(next.date, today)} · ${fmtClock(next.start_min)}`;

  return (
    <section className="up-next" aria-labelledby="up-next-title">
      <div className="up-next-label" id="up-next-title">Up next</div>
      <button type="button" className="up-next-row" onClick={() => onOpenEvent(next)}>
        <CalendarDays size={17} aria-hidden="true" />
        <span className="up-next-copy">
          <span>{when}</span>
          <strong>{next.title}</strong>
        </span>
        <ChevronRight size={18} aria-hidden="true" />
      </button>
    </section>
  );
}
