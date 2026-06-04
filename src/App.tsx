import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "./events";
import { Camera, Check, Download, Mic, Moon, Settings, Smartphone, Square, Sun } from "lucide-react";
import { SettingsModal } from "./Settings";
import { startRecording, type Recorder } from "./audio";
import { fileToImg, type Img } from "./image";
import { useIsMobile } from "./useIsMobile";
import { MobileCapture } from "./MobileCapture";
import { BottomNav, type MobileTab } from "./BottomNav";
import { useTheme } from "./useTheme";
import { api, TokenError, type CategoryInfo, type EntityCandidate, type Envelope, type Health, type NoteRow } from "./api";
import { DataView } from "./DataView";
import { TrendsView } from "./Trends";
import { RecapsView } from "./Recaps";
import { TimelineView } from "./Timeline";
import { PeopleView } from "./PeopleView";
import { PhonePanel } from "./PhonePanel";
import { FloatingChat } from "./FloatingChat";
import { SelfView } from "./Self";
import { TodayView } from "./Today";
import "./App.css";

type Phase = "idle" | "thinking" | "review";
type View = "today" | "log" | "timeline" | "trends" | "recaps" | "people" | "self";
type Source = "text" | "photo";
// One editable review card per extracted entry.
type ReviewCard = {
  catName: string;
  dataText: string;
  description: string;
  routedBy?: "header" | "classifier";
};

// Time-adaptive home greeting: planning prompt in the morning → recap prompt at night.
function homeGreeting(): { dateLine: string; title: string } {
  const now = new Date();
  const h = now.getHours();
  const partOfDay = h < 12 ? "morning" : h < 17 ? "afternoon" : h < 21 ? "evening" : "night";
  const dateStr = now.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" });
  const title =
    h < 11
      ? "What's your schedule looking like today?"
      : h < 17
        ? "How's your day going?"
        : "What did you do today?";
  return { dateLine: `${dateStr} · ${partOfDay}`, title };
}

export default function App() {
  const { theme, toggle } = useTheme();
  const [view, setView] = useState<View>("today");
  const [text, setText] = useState("");
  const [img, setImg] = useState<Img | null>(null);
  const [source, setSource] = useState<Source>("text");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  // review state: one editable card per extracted entry, plus note-level fields
  const [cards, setCards] = useState<ReviewCard[]>([]);
  const [ocrText, setOcrText] = useState(""); // editable transcription (photo path)
  const [eventDate, setEventDate] = useState(""); // canonical day, editable
  const [dateWasExtracted, setDateWasExtracted] = useState(false);
  const [entityChips, setEntityChips] = useState<EntityCandidate[]>([]); // graph entities to confirm

  const [notes, setNotes] = useState<NoteRow[]>([]);
  const [cats, setCats] = useState<CategoryInfo[]>([]);
  const [health, setHealth] = useState<Health | null>(null);

  // Phone capture + backup
  const [showPhone, setShowPhone] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [backupMsg, setBackupMsg] = useState<string | null>(null);
  const [savedMsg, setSavedMsg] = useState<string | null>(null);
  const [needsRepair, setNeedsRepair] = useState(false); // 403 from a stale phone token

  // Mobile-first layout: phone viewports get a bottom-nav + dedicated capture
  // screen instead of the desktop topbar/review flow.
  const isMobile = useIsMobile();
  const [mobileTab, setMobileTab] = useState<MobileTab>("today");

  // Route a thrown error: a TokenError (phone 403) trips the re-pair screen;
  // anything else shows the normal inline error.
  const handleErr = (e: unknown) =>
    e instanceof TokenError ? setNeedsRepair(true) : setError(String(e));

  // Voice (speech-to-text)
  const [voiceReady, setVoiceReady] = useState<boolean | null>(null);
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const [dlModel, setDlModel] = useState(false);
  const recorderRef = useRef<Recorder | null>(null);

  const refresh = async () => {
    const [n, c] = await Promise.all([api.listNotes(), api.listCategories()]);
    setNotes(n);
    setCats(c);
  };

  useEffect(() => {
    api.health().then(setHealth).catch(handleErr);
    refresh().catch(handleErr);
    api.reindex().catch(() => {}); // backfill any notes missing embeddings
    api.voiceStatus().then((s) => setVoiceReady(s.ready)).catch(() => setVoiceReady(false));
  }, []);

  async function ensureVoice(): Promise<boolean> {
    if (voiceReady) return true;
    setDlModel(true);
    try {
      await api.downloadVoiceModel();
      setVoiceReady(true);
      return true;
    } catch (e) {
      setError(`voice model download failed: ${e}`);
      return false;
    } finally {
      setDlModel(false);
    }
  }

  async function onMic() {
    setError(null);
    if (!voiceReady) {
      await ensureVoice();
      return;
    }
    if (recording) {
      // stop + transcribe
      setRecording(false);
      setTranscribing(true);
      try {
        const { b64, sampleRate } = await recorderRef.current!.stop();
        const said = await api.transcribe(b64, sampleRate);
        if (said) setText((t) => (t ? t.trim() + " " : "") + said);
      } catch (e) {
        setError(`transcription failed: ${e}`);
      } finally {
        setTranscribing(false);
        recorderRef.current = null;
      }
    } else {
      try {
        recorderRef.current = await startRecording();
        setRecording(true);
      } catch (e) {
        setError(`couldn't access the microphone: ${e}`);
      }
    }
  }


  // paste an image straight from the clipboard (screenshots etc.)
  useEffect(() => {
    const onPaste = async (e: ClipboardEvent) => {
      const item = Array.from(e.clipboardData?.items ?? []).find((i) =>
        i.type.startsWith("image/")
      );
      const file = item?.getAsFile();
      if (file) {
        e.preventDefault();
        setImg(await fileToImg(file));
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, []);

  const cardParses = useMemo(
    () =>
      cards.map((c) => {
        try {
          return { ok: true as const, value: JSON.parse(c.dataText || "{}") };
        } catch (e) {
          return { ok: false as const, err: (e as Error).message };
        }
      }),
    [cards]
  );
  const allValid = cardParses.every((p) => p.ok);

  const updateCard = (i: number, patch: Partial<ReviewCard>) =>
    setCards((cs) => cs.map((c, idx) => (idx === i ? { ...c, ...patch } : c)));
  const discardCard = (i: number) => {
    const next = cards.filter((_, idx) => idx !== i);
    if (next.length === 0) resetAll();
    else setCards(next);
  };

  const updateEntity = (i: number, patch: Partial<EntityCandidate>) =>
    setEntityChips((es) => es.map((e, idx) => (idx === i ? { ...e, ...patch } : e)));
  const removeEntity = (i: number) =>
    setEntityChips((es) => es.filter((_, idx) => idx !== i));

  async function pickFile(file?: File | null) {
    if (!file) return;
    setError(null);
    setImg(await fileToImg(file));
  }

  function enterReview(env: Envelope, src: Source) {
    setCards(
      env.entries.map((e) => ({
        catName: e.category,
        dataText: JSON.stringify(e.data, null, 2),
        description: e.description,
        routedBy: e.routed_by,
      }))
    );
    setOcrText(env.raw_text ?? "");
    setEventDate(env.event_date ?? "");
    setDateWasExtracted(!!env.date_was_extracted);
    setEntityChips(env.entities ?? []);
    setSource(src);
    setPhase("review");
  }

  async function onCategorizeText() {
    if (!text.trim()) return;
    setError(null);
    setPhase("thinking");
    try {
      enterReview(await api.categorize(text.trim()), "text");
    } catch (e) {
      setError(String(e));
      setPhase("idle");
    }
  }

  async function ingestPhoto(base64: string, ext: string) {
    setView("log");
    setImg({ base64, ext, dataUrl: `data:image/${ext === "jpg" ? "jpeg" : ext};base64,${base64}` });
    setError(null);
    setPhase("thinking");
    try {
      enterReview(await api.categorizePhoto(base64), "photo");
    } catch (e) {
      setError(String(e));
      setPhase("idle");
    }
  }

  async function onCategorizePhoto() {
    if (img) await ingestPhoto(img.base64, img.ext);
  }

  // Photos arriving from the phone capture server.
  useEffect(() => {
    const un = listen<{ path: string }>("photo-received", async (e) => {
      try {
        const { base64, ext } = await api.readInboxImage(e.payload.path);
        await ingestPhoto(base64, ext);
      } catch (err) {
        setError(String(err));
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function onBackup() {
    setBackupMsg("backing up…");
    try {
      const path = await api.exportDb();
      setBackupMsg(`backed up to ${path}`);
    } catch (e) {
      setBackupMsg(`backup failed: ${e}`);
    }
  }

  async function onSave() {
    if (!cards.length || !allValid) return;
    setError(null);
    try {
      let image_path: string | null = null;
      if (source === "photo" && img) {
        image_path = await api.saveImage(img.base64, img.ext);
      }
      const entries = cards.map((c, i) => ({
        category: c.catName.trim().toLowerCase(),
        description: c.description,
        data: (cardParses[i] as { ok: true; value: Record<string, unknown> }).value,
      }));
      const entities = entityChips
        .map((e) => ({
          name: e.name.trim(),
          type: e.type.trim().toLowerCase(),
          fact: e.fact?.trim() || undefined,
          relationship: e.relationship?.trim() || undefined,
        }))
        .filter((e) => e.name && e.type);
      await api.save({
        raw_text: source === "photo" ? ocrText : text.trim(),
        source,
        image_path,
        event_date: eventDate,
        entries,
        entities,
      });
      const savedCats = Array.from(new Set(entries.map((e) => e.category))).join(", ");
      resetAll();
      await refresh();
      setSavedMsg(savedCats);
      setTimeout(() => setSavedMsg(null), 4000);
    } catch (e) {
      setError(String(e));
    }
  }

  function resetAll() {
    setText("");
    setImg(null);
    setSource("text");
    setCards([]);
    setOcrText("");
    setEventDate("");
    setDateWasExtracted(false);
    setEntityChips([]);
    setPhase("idle");
  }

  const busy = phase === "thinking";

  // Phone lost its access token (403) — show a recoverable re-pair screen
  // instead of a dead app. Relaunching from the home-screen icon (whose URL
  // carries the token) clears this on next load.
  if (needsRepair) {
    return (
      <div className="app repair-screen">
        <div className="repair-card">
          <div className="brand">noted<span className="dot">.</span></div>
          <h2>Reconnect to your Mac</h2>
          <p>
            This phone’s connection expired. Open <strong>noted</strong> on your Mac, click the
            phone button, and scan the QR code again — or just relaunch noted from your Home Screen
            icon.
          </p>
          <button className="primary" onClick={() => window.location.reload()}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  // Mobile-first shell: slim top bar + bottom nav + dedicated capture screen.
  // The desktop layout below is left entirely untouched.
  if (isMobile) {
    return (
      <div className="app mobile">
        <header className="mobile-topbar">
          <div className="brand">
            noted<span className="dot">.</span>
          </div>
          <button className="icon-btn" onClick={() => setShowSettings(true)} aria-label="Settings">
            <Settings size={18} />
          </button>
        </header>

        <main className="mobile-content">
          {mobileTab === "today" && (
            <TodayView notes={notes} onMakeSchedule={() => setMobileTab("capture")} />
          )}
          {mobileTab === "capture" && <MobileCapture onCaptured={() => refresh().catch(handleErr)} />}
          {mobileTab === "timeline" && <TimelineView notes={notes} />}
          {mobileTab === "recaps" && <RecapsView />}
        </main>

        <BottomNav
          active={mobileTab}
          onChange={(t) => {
            setMobileTab(t);
            // Re-fetch when navigating to a list/schedule so a freshly auto-filed note shows.
            if (t === "today" || t === "timeline" || t === "recaps") refresh().catch(handleErr);
          }}
        />

        <FloatingChat
          variant="sheet"
          open={mobileTab === "ask"}
          onOpenChange={(o) => {
            if (!o) setMobileTab("capture");
          }}
          onMutated={() => refresh().catch(handleErr)}
        />

        {showSettings && <SettingsModal onClose={() => setShowSettings(false)} />}
      </div>
    );
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          noted<span className="dot">.</span>
        </div>
        <nav className="nav">
          <button className={view === "today" ? "on" : ""} onClick={() => setView("today")}>
            Today
          </button>
          <button className={view === "log" ? "on" : ""} onClick={() => setView("log")}>
            Log
          </button>
          <button className={view === "timeline" ? "on" : ""} onClick={() => setView("timeline")}>
            Timeline
          </button>
          <button className={view === "trends" ? "on" : ""} onClick={() => setView("trends")}>
            Trends
          </button>
          <button className={view === "recaps" ? "on" : ""} onClick={() => setView("recaps")}>
            Recaps
          </button>
          <button className={view === "people" ? "on" : ""} onClick={() => setView("people")}>
            People
          </button>
          <button className={view === "self" ? "on" : ""} onClick={() => setView("self")}>
            Self
          </button>
        </nav>
        <span className="spacer" />
        <button
          className="icon-btn"
          onClick={toggle}
          title={theme === "dark" ? "Switch to light" : "Switch to dark"}
          aria-label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
        >
          {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
        </button>
        <button className="icon-btn" onClick={() => setShowPhone(true)} title="Capture from your phone">
          <Smartphone size={18} />
        </button>
        <button className="icon-btn" onClick={() => setShowSettings(true)} title="Models & settings">
          <Settings size={18} />
        </button>
      </header>

      <main className="content">
        {view === "today" ? (
          <TodayView notes={notes} onMakeSchedule={() => setView("log")} />
        ) : view === "trends" ? (
          <TrendsView cats={cats} theme={theme} />
        ) : view === "recaps" ? (
          <RecapsView />
        ) : view === "people" ? (
          <PeopleView />
        ) : view === "self" ? (
          <SelfView theme={theme} />
        ) : view === "timeline" ? (
          <TimelineView notes={notes} />
        ) : (
        <div className="log-hero">
        <div className="greeting">
          <div className="greet-date">{homeGreeting().dateLine}</div>
          <h1 className="greet-title">{homeGreeting().title}</h1>
        </div>
        <section
          className={"capture" + (dragOver ? " dragover" : "")}
          onDragOver={(e) => {
            e.preventDefault();
            setDragOver(true);
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragOver(false);
            pickFile(e.dataTransfer.files?.[0]);
          }}
        >
          {img ? (
            <div className="img-attached">
              <img src={img.dataUrl} alt="note" />
              <div className="img-meta">
                <span>photo attached</span>
                <button className="link" onClick={() => setImg(null)} disabled={busy}>
                  remove
                </button>
              </div>
            </div>
          ) : (
            <textarea
              value={text}
              placeholder="Brain-dump anything — your gym session, today's schedule, what you ate. Or drop / paste a photo of a handwritten note."
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) onCategorizeText();
              }}
              disabled={busy}
            />
          )}

          <input
            ref={fileInput}
            type="file"
            accept="image/*"
            hidden
            onChange={(e) => pickFile(e.target.files?.[0])}
          />

          <div className="capture-actions">
            {!img && (
              <button className="ghost" onClick={() => fileInput.current?.click()} disabled={busy}>
                <Camera size={15} /> Photo
              </button>
            )}
            {!img && (
              <button
                className={"ghost" + (recording ? " recording" : "")}
                onClick={onMic}
                disabled={busy || transcribing || dlModel}
                title="Talk through your day"
              >
                {recording ? <Square size={14} strokeWidth={2.5} /> : <Mic size={15} />}
                {dlModel
                  ? "Downloading…"
                  : transcribing
                    ? "Transcribing…"
                    : recording
                      ? "Stop"
                      : voiceReady === false
                        ? "Enable voice"
                        : "Speak"}
              </button>
            )}
            <span className="hint">{img ? "review the transcription next" : "⌘↵ to file"}</span>
            {img ? (
              <button className="primary" onClick={onCategorizePhoto} disabled={busy}>
                {busy ? "Reading…" : "Read photo"}
              </button>
            ) : (
              <button className="primary" onClick={onCategorizeText} disabled={busy || !text.trim()}>
                {busy ? "Reading…" : "File it"}
              </button>
            )}
          </div>
        </section>

        {error && <div className="error">{error}</div>}

        {phase === "review" && cards.length > 0 && (
          <section className="review">
            <div className="review-head">
              <div className="cat-edit date-edit">
                <label>Date</label>
                <input type="date" value={eventDate} onChange={(e) => setEventDate(e.target.value)} />
                <span className={"badge " + (dateWasExtracted ? "existing" : "new")}>
                  {dateWasExtracted ? "from note" : "today"}
                </span>
              </div>
              <span className="review-count">
                {cards.length} {cards.length === 1 ? "entry" : "entries"} from this note
              </span>
            </div>

            {source === "photo" && (
              <div className="transcription">
                <label>Transcription — fix any misreads</label>
                <div className="trans-row">
                  {img && <img className="trans-thumb" src={img.dataUrl} alt="note" />}
                  <textarea value={ocrText} onChange={(e) => setOcrText(e.target.value)} spellCheck={false} />
                </div>
              </div>
            )}

            <div className="review-cards">
              {cards.map((card, i) => {
                const known = cats.find((c) => c.name === card.catName.trim().toLowerCase());
                const parse = cardParses[i];
                return (
                  <div className="entry-card" key={i}>
                    <div className="entry-head">
                      <div className="cat-edit">
                        <label>Category</label>
                        <input
                          value={card.catName}
                          onChange={(e) => updateCard(i, { catName: e.target.value })}
                        />
                        {known ? (
                          <span className="badge existing">{known.entry_count} existing</span>
                        ) : (
                          <span className="badge new">new category</span>
                        )}
                        {card.routedBy === "header" && (
                          <span className="badge routed" title="You tagged this section with a header">
                            tagged
                          </span>
                        )}
                      </div>
                      {cards.length > 1 && (
                        <button className="link" onClick={() => discardCard(i)}>
                          discard
                        </button>
                      )}
                    </div>
                    {card.description && <p className="desc">{card.description}</p>}

                    <div className="review-body">
                      <div className="pane">
                        <label>Extracted data {parse.ok ? "" : "⚠︎ invalid JSON"}</label>
                        <textarea
                          className={"json " + (parse.ok ? "" : "bad")}
                          value={card.dataText}
                          onChange={(e) => updateCard(i, { dataText: e.target.value })}
                          spellCheck={false}
                        />
                      </div>
                      <div className="pane preview">
                        <label>Preview</label>
                        <div className="preview-box">
                          {parse.ok ? (
                            <DataView value={parse.value} />
                          ) : (
                            <span className="muted">fix JSON to preview</span>
                          )}
                        </div>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>

            {entityChips.length > 0 && (
              <div className="entities-review">
                <label>Entities — people, places &amp; things in this note (confirm or remove)</label>
                <div className="entity-chips">
                  {entityChips.map((e, i) => (
                    <span className="entity-chip" key={i}>
                      <input
                        className="entity-name"
                        value={e.name}
                        onChange={(ev) => updateEntity(i, { name: ev.target.value })}
                        spellCheck={false}
                      />
                      <select
                        className="entity-type"
                        value={e.type}
                        onChange={(ev) => updateEntity(i, { type: ev.target.value })}
                      >
                        {Array.from(
                          new Set(["person", "place", "activity", "food", "item", "org", "topic", e.type])
                        )
                          .filter(Boolean)
                          .map((t) => (
                            <option key={t} value={t}>
                              {t}
                            </option>
                          ))}
                      </select>
                      {e.type === "person" && (e.relationship || e.fact) && (
                        <span className="entity-fact" title="captured about this person">
                          {e.relationship ? `${e.relationship}` : ""}
                          {e.relationship && e.fact ? " · " : ""}
                          {e.fact ?? ""}
                        </span>
                      )}
                      <button className="entity-x" onClick={() => removeEntity(i)} title="remove">
                        ×
                      </button>
                    </span>
                  ))}
                </div>
              </div>
            )}

            <div className="review-actions">
              <button onClick={resetAll}>Discard all</button>
              <button className="primary" onClick={onSave} disabled={!allValid}>
                {cards.length > 1
                  ? `Save ${cards.length} entries`
                  : `Save to ${cards[0].catName.trim().toLowerCase() || "…"}`}
              </button>
            </div>
          </section>
        )}

        {savedMsg && (
          <button className="saved-toast" onClick={() => setView("timeline")}>
            <Check size={15} /> Filed under <strong>{savedMsg}</strong> · view timeline
          </button>
        )}
        </div>
        )}
      </main>

      <footer className="status">
        <span className="meta">
          <span className="live-dot" />
          {health
            ? `${health.models.length} local models · ${
                health.models.some((m) => m.startsWith("qwen2.5vl")) ? "vision ready" : "no vision model"
              }`
            : "connecting to local models…"}
        </span>
        <span className="meta">
          {backupMsg && <span className="backup-msg">{backupMsg}</span>}
          <button className="link" onClick={onBackup}>
            <Download size={14} /> Back up
          </button>
        </span>
      </footer>

      {showPhone && <PhonePanel onClose={() => setShowPhone(false)} />}
      {showSettings && <SettingsModal onClose={() => setShowSettings(false)} />}
      <FloatingChat onMutated={refresh} />
    </div>
  );
}
