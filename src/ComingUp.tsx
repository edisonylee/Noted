// "Coming up" (Granola's habit loop): the next few calendar meetings across
// every connected account, each row a click away from its meeting page, plus
// the most recent recorded meetings. 5 per page, paging arrows, quiet when
// there's nothing — the strip should never shout.

import { useCallback, useEffect, useMemo, useState } from "react";
import { AudioLines, ChevronLeft, ChevronRight, FileText, Loader, Mic, Users, Video } from "lucide-react";
import { api, type MeetingListRow, type RangeEvent } from "./api";
import { easternDay, easternMinutes, relativeDay } from "./day";
import { joinUrl } from "./joinUrl";

const PAGE = 5;

function fmtClock(min: number | null): string {
  if (min == null) return "";
  const h = Math.floor((min % 1440) / 60);
  const m = min % 60;
  const ampm = h >= 12 ? "pm" : "am";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return m === 0 ? `${h12}${ampm}` : `${h12}:${String(m).padStart(2, "0")}${ampm}`;
}

export function ComingUp({
  onOpenEvent,
  onOpenMeeting,
  activeMeetingId,
  refreshKey,
}: {
  onOpenEvent: (ev: RangeEvent) => void;
  onOpenMeeting: (id: number) => void;
  activeMeetingId: number | null;
  refreshKey?: number;
}) {
  const [events, setEvents] = useState<RangeEvent[] | null>(null);
  const [meetings, setMeetings] = useState<MeetingListRow[]>([]);
  const [page, setPage] = useState(0);
  const [nowMin, setNowMin] = useState(easternMinutes());

  const load = useCallback(async () => {
    const today = easternDay();
    const tomorrow = easternDay(new Date(Date.now() + 86_400_000));
    try {
      const [evs, ms] = await Promise.all([
        api.gcalEventsRange(today, tomorrow),
        api.meetingList(),
      ]);
      setEvents(evs);
      setMeetings(ms);
    } catch {
      setEvents([]); // no calendar connected — the strip just shows recordings
      api.meetingList().then(setMeetings).catch(() => {});
    }
  }, []);

  useEffect(() => {
    load();
    const t = window.setInterval(() => {
      setNowMin(easternMinutes());
      load();
    }, 300_000);
    return () => window.clearInterval(t);
  }, [load, refreshKey]);

  const upcoming = useMemo(() => {
    if (!events) return [];
    const today = easternDay();
    const recordedEventIds = new Set(
      meetings.map((m) => m.event_json?.id).filter((id): id is string => Boolean(id))
    );
    return events
      .filter((e) => !e.declined && !e.all_day && e.start_min != null)
      // Meetings only — a call link or other people. Plain calendar blocks
      // (including noted's own pushed schedule/tasks) stay in the schedule
      // below; this strip is for things that get recorded.
      .filter((e) => e.calendar.toLowerCase() !== "noted")
      .filter((e) => !recordedEventIds.has(e.id))
      .filter((e) => e.meet_link != null || e.attendee_count >= 2)
      .filter((e) => {
        // Still relevant: hasn't ended yet (with 5 min grace) or is tomorrow.
        if (e.date !== today) return true;
        const end = e.end_min ?? (e.start_min ?? 0) + 60;
        return end > nowMin - 5;
      })
      .sort((a, b) => (a.date === b.date ? (a.start_min ?? 0) - (b.start_min ?? 0) : a.date < b.date ? -1 : 1));
  }, [events, meetings, nowMin]);

  const pages = Math.max(1, Math.ceil(upcoming.length / PAGE));
  const view = upcoming.slice(page * PAGE, page * PAGE + PAGE);
  // Recent recordings: failed/empty attempts have nothing to open — hide them.
  // (Still recording / summarizing / done all carry something to show.)
  const recent = meetings.filter((m) => m.status !== "failed").slice(0, 4);
  const today = easternDay();

  if (events === null) return null; // first load: no flash
  if (upcoming.length === 0 && recent.length === 0) return null; // nothing to say

  return (
    <section className="comingup">
      {upcoming.length > 0 && (
        <>
          <div className="comingup-head">
            <h3>Coming up</h3>
            <span className="spacer" />
            {pages > 1 && (
              <span className="comingup-pager">
                <button
                  className="icon-btn"
                  disabled={page === 0}
                  onClick={() => setPage((p) => p - 1)}
                  aria-label="Earlier meetings"
                >
                  <ChevronLeft size={15} />
                </button>
                <button
                  className="icon-btn"
                  disabled={page >= pages - 1}
                  onClick={() => setPage((p) => p + 1)}
                  aria-label="Later meetings"
                >
                  <ChevronRight size={15} />
                </button>
              </span>
            )}
          </div>
          <div className="comingup-list">
            {view.map((ev) => {
              const live =
                ev.date === today &&
                ev.start_min != null &&
                ev.start_min <= nowMin &&
                (ev.end_min ?? ev.start_min + 60) > nowMin;
              return (
                // A div, not a <button>: the Join anchor lives inside, and
                // nested interactive content inside a button is invalid HTML —
                // WebKit swallows the anchor's click.
                <div
                  key={ev.id}
                  className={"comingup-row" + (live ? " live" : "")}
                  role="button"
                  tabIndex={0}
                  onClick={() => onOpenEvent(ev)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") onOpenEvent(ev);
                  }}
                >
                  <span className="cal-dot" style={{ background: ev.color }} />
                  <span className="cu-time">
                    {ev.date !== today ? "Tmrw " : ""}
                    {fmtClock(ev.start_min)}
                  </span>
                  <span className="cu-title">{ev.title}</span>
                  {ev.attendee_count > 1 && (
                    <span className="cu-meta" title={`${ev.attendee_count} attendees — noted will offer to record`}>
                      <Users size={12} /> {ev.attendee_count}
                      <Mic size={12} className="will-record" />
                    </span>
                  )}
                  {ev.meet_link && (
                    <a
                      className="cu-join"
                      href={joinUrl(ev.meet_link, ev.account)}
                      target="_blank"
                      rel="noreferrer"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <Video size={13} /> Join
                    </a>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}

      {recent.length > 0 && (
        <div className="comingup-recent">
          {/* Labeled so recordings never read as (stale) calendar events. */}
          <div className="comingup-head">
            <h3>Recent recordings</h3>
          </div>
          {recent.map((m) => (
            <button key={m.id} className="recent-row" onClick={() => onOpenMeeting(m.id)}>
              {m.status === "recording" || m.id === activeMeetingId ? (
                <span className="bars small" aria-hidden>
                  <i />
                  <i />
                  <i />
                </span>
              ) : m.status === "summarizing" ? (
                <Loader size={13} className="spin" />
              ) : m.summary_count > 0 ? (
                <FileText size={13} />
              ) : (
                <AudioLines size={13} />
              )}
              <span className="cu-title">{m.title}</span>
              <span className="cu-when">
                {m.started_at ? relativeDay(m.started_at.slice(0, 10)) : ""}
              </span>
            </button>
          ))}
        </div>
      )}
    </section>
  );
}
