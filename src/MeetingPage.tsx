// The meeting page: pre-meeting prep → live recording (notes front and center,
// transcript hidden behind a toggle, Granola-style) → summary tabs (PLAUD's
// multidimensional model: "+" regenerates with another template as a new tab).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  AudioLines,
  Check,
  ChevronDown,
  Loader,
  Mic,
  Plus,
  Square,
  Users,
  Video,
} from "lucide-react";
import { listen } from "./events";
import {
  api,
  type MeetingDetail,
  type MeetingSegment,
  type MeetingTemplate,
  type RangeEvent,
} from "./api";

function mmss(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

function fmtClock(min: number | null): string {
  if (min == null) return "";
  const h = Math.floor((min % 1440) / 60);
  const m = min % 60;
  const ampm = h >= 12 ? "pm" : "am";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return m === 0 ? `${h12}${ampm}` : `${h12}:${String(m).padStart(2, "0")}${ampm}`;
}

// Minimal markdown for our own deterministic summary output (## headings,
// bullets, checkboxes, **bold**) — no external renderer dependency.
function MdBlock({ md }: { md: string }) {
  const bold = (s: string) => {
    const parts = s.split(/\*\*([^*]+)\*\*/g);
    return parts.map((p, i) => (i % 2 === 1 ? <strong key={i}>{p}</strong> : p));
  };
  const lines = md.split("\n");
  const out: React.ReactNode[] = [];
  let list: React.ReactNode[] = [];
  let key = 0;
  const flush = () => {
    if (list.length) {
      out.push(<ul key={key++}>{list}</ul>);
      list = [];
    }
  };
  for (const line of lines) {
    const t = line.trim();
    if (t.startsWith("## ")) {
      flush();
      out.push(<h3 key={key++}>{t.slice(3)}</h3>);
    } else if (t.startsWith("- [ ] ") || t.startsWith("- [x] ")) {
      list.push(
        <li key={key++} className="todo">
          <span className="box">{t[3] === "x" ? <Check size={11} /> : null}</span>
          {bold(t.slice(6))}
        </li>
      );
    } else if (t.startsWith("- ")) {
      list.push(<li key={key++}>{bold(t.slice(2))}</li>);
    } else if (t === "") {
      flush();
    } else {
      flush();
      out.push(<p key={key++}>{bold(t)}</p>);
    }
  }
  flush();
  return <div className="md">{out}</div>;
}

type Tab = "notes" | "transcript" | number; // number = summary index

export function MeetingPage({
  id,
  event,
  onBack,
  onStarted,
}: {
  id: number | null; // null = pre-meeting page for a calendar event
  event?: Partial<RangeEvent> | null;
  onBack: () => void;
  onStarted?: (id: number) => void;
}) {
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [liveSegments, setLiveSegments] = useState<MeetingSegment[]>([]);
  const [notes, setNotes] = useState("");
  const [tab, setTab] = useState<Tab>("notes");
  const [elapsed, setElapsed] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [templates, setTemplates] = useState<MeetingTemplate[]>([]);
  const [pickTemplate, setPickTemplate] = useState(false);
  const [generating, setGenerating] = useState<string | null>(null);
  const notesTimer = useRef<number | null>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);

  const recording = detail?.status === "recording";
  const summarizing = detail?.status === "summarizing" || generating != null;

  const load = useCallback(async () => {
    if (id == null) return;
    try {
      const d = await api.meetingGet(id);
      setDetail(d);
      setLiveSegments(d.segments);
      setNotes((prev) => (prev === "" ? d.raw_notes : prev));
      // First summary that appears: switch to it once.
      setTab((t) => (t === "notes" && d.summaries.length > 0 && d.status === "done" ? 0 : t));
    } catch (e) {
      setError(String(e));
    }
  }, [id]);

  useEffect(() => {
    load();
    api.meetingTemplates().then(setTemplates).catch(() => {});
  }, [load]);

  // Live updates: segments stream in; stopped/summarized refresh the page.
  useEffect(() => {
    if (id == null) return;
    const subs = [
      listen<MeetingSegment & { meetingId: number }>("meeting-segment", (e) => {
        if (e.payload.meetingId !== id) return;
        setLiveSegments((prev) => [...prev, e.payload]);
      }),
      // A "me" line recognized late as the mic hearing the speakers (echo of
      // remote speech) gets deleted by the worker — drop it from the view too.
      listen<{ meetingId: number; id: number }>("meeting-segment-removed", (e) => {
        if (e.payload.meetingId !== id) return;
        setLiveSegments((prev) => prev.filter((s) => s.id !== e.payload.id));
      }),
      listen<{ meetingId: number }>("meeting-stopped", (e) => {
        if (e.payload.meetingId === id) load();
      }),
      listen<{ meetingId: number }>("meeting-summarized", (e) => {
        if (e.payload.meetingId === id) load();
      }),
    ];
    return () => {
      subs.forEach((p) => p.then((un) => un()));
    };
  }, [id, load]);

  // Recording clock.
  useEffect(() => {
    if (!recording) return;
    const started = detail?.started_at ? Date.parse(detail.started_at) : Date.now();
    const t = window.setInterval(() => setElapsed(Date.now() - started), 1000);
    return () => window.clearInterval(t);
  }, [recording, detail?.started_at]);

  // Notes autosave (debounced) once a meeting row exists.
  const onNotes = (v: string) => {
    setNotes(v);
    if (id == null) return; // pre-meeting: passed along at start
    if (notesTimer.current) window.clearTimeout(notesTimer.current);
    notesTimer.current = window.setTimeout(() => {
      api.meetingSetNotes(id, v).catch(() => {});
    }, 800);
  };

  // Keep the live transcript pinned to the newest line when already at bottom.
  useEffect(() => {
    const el = transcriptRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [liveSegments.length]);

  const ev = detail?.event_json ?? event ?? null;
  const title = detail?.title ?? ev?.title ?? "Meeting";
  const attendees = (ev?.attendees ?? []).filter((a) => !a.self);
  const meetLink = ev?.meet_link ?? null;

  const notesRef = useRef(notes);
  notesRef.current = notes;

  const start = useCallback(async () => {
    setStarting(true);
    setError(null);
    try {
      const newId = await api.meetingStart({
        title,
        eventId: ev?.id ?? undefined,
        eventJson: ev ?? undefined,
      });
      const prep = notesRef.current;
      if (prep.trim()) await api.meetingSetNotes(newId, prep).catch(() => {});
      onStarted?.(newId);
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [title, ev?.id]);

  // Deliberately NO auto-start here. Recording begins only from an explicit
  // click — this page's Record button or the detection prompt (which fires at
  // T-60s even when this page is open, so nothing is lost). Ending is the
  // automatic half: the detector stops when the call app releases the mic.

  const stop = async () => {
    setStopping(true);
    setError(null);
    try {
      await api.meetingStop();
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setStopping(false);
    }
  };

  const generate = async (template: string) => {
    if (id == null) return;
    setPickTemplate(false);
    setGenerating(template);
    setError(null);
    try {
      await api.meetingSummarize(id, template);
      await load();
      setTab((detail?.summaries.length ?? 0)); // the tab just added
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(null);
    }
  };

  const talkPct = useMemo(() => {
    const me = detail?.talk_ms.me ?? 0;
    const them = detail?.talk_ms.them ?? 0;
    const total = me + them;
    return total > 0 ? Math.round((me / total) * 100) : null;
  }, [detail?.talk_ms]);

  const summaries = detail?.summaries ?? [];

  return (
    <div className="meeting-page">
      <header className="meeting-head">
        <button className="icon-btn" onClick={onBack} title="Back" aria-label="Back">
          <ArrowLeft size={18} />
        </button>
        <div className="meeting-title">
          <h2>{title}</h2>
          <div className="meeting-meta">
            {ev?.start_min != null && (
              <span>
                {fmtClock(ev.start_min)}
                {ev.end_min != null ? `–${fmtClock(ev.end_min)}` : ""}
              </span>
            )}
            {attendees.length > 0 && (
              <span title={attendees.map((a) => a.name || a.email).join(", ")}>
                <Users size={13} /> {ev?.attendee_count ?? attendees.length}
              </span>
            )}
            {talkPct != null && !recording && <span>you spoke {talkPct}%</span>}
          </div>
        </div>
        <span className="spacer" />
        {meetLink && (
          <a className="btn ghost" href={meetLink} target="_blank" rel="noreferrer">
            <Video size={15} /> Join
          </a>
        )}
        {id == null ? (
          <button className="btn rec" onClick={start} disabled={starting}>
            {starting ? <Loader size={15} className="spin" /> : <Mic size={15} />} Record
          </button>
        ) : recording ? (
          <button className="btn stop" onClick={stop} disabled={stopping}>
            {stopping ? <Loader size={15} className="spin" /> : <Square size={13} />}
            <span className="bars" aria-hidden>
              <i />
              <i />
              <i />
            </span>
            {mmss(elapsed)}
          </button>
        ) : summarizing ? (
          <span className="meeting-status">
            <Loader size={14} className="spin" /> enhancing notes…
          </span>
        ) : null}
      </header>

      {error && <div className="error">{error}</div>}
      {id == null && (
        <p className="meeting-hint">
          Jot prep notes below — recording starts when you hit Record, and your notes get
          expanded with the transcript afterwards.
        </p>
      )}

      <nav className="meeting-tabs">
        <button className={tab === "notes" ? "on" : ""} onClick={() => setTab("notes")}>
          Notes
        </button>
        {id != null && (
          <button
            className={tab === "transcript" ? "on" : ""}
            onClick={() => setTab("transcript")}
          >
            <AudioLines size={13} /> Transcript
            {liveSegments.length > 0 ? ` (${liveSegments.length})` : ""}
          </button>
        )}
        {summaries.map((s, i) => (
          <button key={s.id} className={tab === i ? "on" : ""} onClick={() => setTab(i)}>
            {s.template}
          </button>
        ))}
        {id != null && !recording && liveSegments.length > 0 && (
          <div className="tab-add">
            <button
              onClick={() => setPickTemplate((v) => !v)}
              disabled={generating != null}
              title="Generate a summary with a template"
            >
              {generating ? <Loader size={13} className="spin" /> : <Plus size={13} />}
              {summaries.length === 0 ? " Summarize" : ""}
              <ChevronDown size={12} />
            </button>
            {pickTemplate && (
              <div className="tab-menu">
                {templates.map((t) => (
                  <button key={t.name} onClick={() => generate(t.name)}>
                    {t.name}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </nav>

      {tab === "notes" ? (
        <textarea
          className="meeting-notes"
          value={notes}
          onChange={(e) => onNotes(e.target.value)}
          placeholder={
            recording
              ? "Terse trigger bullets are enough — they'll be expanded with the transcript when the meeting ends."
              : "Notes for this meeting…"
          }
        />
      ) : tab === "transcript" ? (
        <div className="meeting-transcript" ref={transcriptRef}>
          {liveSegments.length === 0 ? (
            <p className="quiet-empty">
              {recording ? "Listening — the transcript fills in as people speak." : "No transcript."}
            </p>
          ) : (
            [...liveSegments]
              .sort((a, b) => a.t0_ms - b.t0_ms || a.id - b.id)
              .map((s) => (
                <div key={s.id} className={"bubble " + s.channel}>
                  <span className="who">
                    {s.channel === "me" ? "Me" : s.speaker || "Them"} · {mmss(s.t0_ms)}
                  </span>
                  <p>{s.text}</p>
                </div>
              ))
          )}
        </div>
      ) : (
        <div className="meeting-summary">
          {summaries[tab as number] ? <MdBlock md={summaries[tab as number].content_md} /> : null}
        </div>
      )}
    </div>
  );
}
