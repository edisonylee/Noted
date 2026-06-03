import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { startRecording, type Recorder } from "./audio";
import { api, type AskSource, type CategoryInfo, type Health, type NoteRow, type Proposal } from "./api";
import { DataView } from "./DataView";
import { TrendsView } from "./Trends";
import { RecapsView } from "./Recaps";
import { PhonePanel } from "./PhonePanel";
import "./App.css";

type Phase = "idle" | "thinking" | "review";
type View = "log" | "trends" | "recaps";
type Source = "text" | "photo";
type Img = { base64: string; ext: string; dataUrl: string };

async function fileToImg(file: File): Promise<Img> {
  const dataUrl: string = await new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(r.result as string);
    r.onerror = rej;
    r.readAsDataURL(file);
  });
  const base64 = dataUrl.split(",")[1] ?? "";
  const mime = dataUrl.slice(5, dataUrl.indexOf(";"));
  const ext = mime.includes("jpeg") ? "jpg" : mime.split("/")[1] || "png";
  return { base64, ext, dataUrl };
}

export default function App() {
  const [view, setView] = useState<View>("log");
  const [text, setText] = useState("");
  const [img, setImg] = useState<Img | null>(null);
  const [source, setSource] = useState<Source>("text");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  // review state (editable proposal)
  const [proposal, setProposal] = useState<Proposal | null>(null);
  const [catName, setCatName] = useState("");
  const [dataText, setDataText] = useState("");
  const [ocrText, setOcrText] = useState(""); // editable transcription (photo path)
  const [eventDate, setEventDate] = useState(""); // canonical day, editable

  const [notes, setNotes] = useState<NoteRow[]>([]);
  const [cats, setCats] = useState<CategoryInfo[]>([]);
  const [health, setHealth] = useState<Health | null>(null);

  // Ask my notes — conversational voice assistant
  type ChatMessage = { role: "user" | "assistant"; content: string; sources?: AskSource[] };
  const [askQ, setAskQ] = useState("");
  const [asking, setAsking] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [muted, setMuted] = useState(false);
  const [chatRecording, setChatRecording] = useState(false);
  const chatRecorderRef = useRef<Recorder | null>(null);

  // Phone capture + backup
  const [showPhone, setShowPhone] = useState(false);
  const [backupMsg, setBackupMsg] = useState<string | null>(null);

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
    api.health().then(setHealth).catch((e) => setError(String(e)));
    refresh().catch((e) => setError(String(e)));
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

  async function sendMessage(q: string) {
    const question = q.trim();
    if (!question || asking) return;
    setError(null);
    setAskQ("");
    const history = messages.map((m) => ({ role: m.role, content: m.content }));
    setMessages((prev) => [...prev, { role: "user", content: question }]);
    setAsking(true);
    try {
      const res = await api.chat(question, history);
      setMessages((prev) => [...prev, { role: "assistant", content: res.answer, sources: res.sources }]);
      if (!muted) api.speak(res.answer).catch(() => {});
    } catch (e) {
      setError(String(e));
    } finally {
      setAsking(false);
    }
  }

  // Ask by voice: record -> transcribe -> send as the next turn.
  async function onChatMic() {
    setError(null);
    if (!(await ensureVoice())) return;
    if (chatRecording) {
      setChatRecording(false);
      setAsking(true);
      try {
        const { b64, sampleRate } = await chatRecorderRef.current!.stop();
        const q = await api.transcribe(b64, sampleRate);
        setAsking(false);
        if (q) await sendMessage(q);
      } catch (e) {
        setError(`voice question failed: ${e}`);
        setAsking(false);
      } finally {
        chatRecorderRef.current = null;
      }
    } else {
      try {
        chatRecorderRef.current = await startRecording();
        setChatRecording(true);
      } catch (e) {
        setError(`couldn't access the microphone: ${e}`);
      }
    }
  }

  function toggleMute() {
    setMuted((m) => {
      if (!m) api.stopSpeaking().catch(() => {});
      return !m;
    });
  }

  function clearChat() {
    setMessages([]);
    api.stopSpeaking().catch(() => {});
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

  const dataParse = useMemo(() => {
    try {
      return { ok: true as const, value: JSON.parse(dataText || "{}") };
    } catch (e) {
      return { ok: false as const, err: (e as Error).message };
    }
  }, [dataText]);

  async function pickFile(file?: File | null) {
    if (!file) return;
    setError(null);
    setImg(await fileToImg(file));
  }

  function enterReview(p: Proposal, src: Source) {
    setProposal(p);
    setCatName(p.category);
    setDataText(JSON.stringify(p.data, null, 2));
    setOcrText(p.raw_text ?? "");
    setEventDate(p.event_date ?? "");
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
    if (!proposal || !dataParse.ok) return;
    setError(null);
    try {
      let image_path: string | null = null;
      if (source === "photo" && img) {
        image_path = await api.saveImage(img.base64, img.ext);
      }
      await api.save({
        raw_text: source === "photo" ? ocrText : text.trim(),
        source,
        image_path,
        category: catName.trim().toLowerCase(),
        description: proposal.description,
        event_date: eventDate,
        data: dataParse.value,
      });
      resetAll();
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  function resetAll() {
    setText("");
    setImg(null);
    setSource("text");
    setProposal(null);
    setDataText("");
    setOcrText("");
    setEventDate("");
    setCatName("");
    setPhase("idle");
  }

  const knownCat = cats.find((c) => c.name === catName.trim().toLowerCase());
  const willCreate = !knownCat;
  const busy = phase === "thinking";

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          noted<span className="dot">.</span>
        </div>
        <nav className="nav">
          <button className={view === "log" ? "on" : ""} onClick={() => setView("log")}>
            Log
          </button>
          <button className={view === "trends" ? "on" : ""} onClick={() => setView("trends")}>
            Trends
          </button>
          <button className={view === "recaps" ? "on" : ""} onClick={() => setView("recaps")}>
            Recaps
          </button>
        </nav>
        <button className="phone-btn" onClick={() => setShowPhone(true)} title="Capture from your phone">
          📱
        </button>
        <div className="cats">
          {cats.map((c) => (
            <span className="chip" key={c.id} title={c.description}>
              {c.name}
              <em>{c.entry_count}</em>
            </span>
          ))}
        </div>
      </header>

      <main className="content">
        {view === "trends" ? (
          <TrendsView cats={cats} />
        ) : view === "recaps" ? (
          <RecapsView />
        ) : (
        <>
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
                📷 Photo
              </button>
            )}
            {!img && (
              <button
                className={"ghost mic" + (recording ? " recording" : "")}
                onClick={onMic}
                disabled={busy || transcribing || dlModel}
                title="Talk through your day"
              >
                {dlModel
                  ? "Downloading voice…"
                  : transcribing
                    ? "Transcribing…"
                    : recording
                      ? "⏹ Stop"
                      : voiceReady === false
                        ? "🎙 Enable voice"
                        : "🎙 Speak"}
              </button>
            )}
            <span className="hint">{img ? "check the transcription on the next step" : "⌘↵ to file it"}</span>
            {img ? (
              <button className="primary" onClick={onCategorizePhoto} disabled={busy}>
                {busy ? "Reading photo…" : "Read photo"}
              </button>
            ) : (
              <button className="primary" onClick={onCategorizeText} disabled={busy || !text.trim()}>
                {busy ? "Reading…" : "File it"}
              </button>
            )}
          </div>
        </section>

        {error && <div className="error">{error}</div>}

        {phase === "review" && proposal && (
          <section className="review">
            <div className="review-head">
              <div className="cat-edit">
                <label>Category</label>
                <input value={catName} onChange={(e) => setCatName(e.target.value)} />
                {willCreate ? (
                  <span className="badge new">new category</span>
                ) : (
                  <span className="badge existing">{knownCat?.entry_count} existing</span>
                )}
              </div>
              <div className="cat-edit date-edit">
                <label>Date</label>
                <input type="date" value={eventDate} onChange={(e) => setEventDate(e.target.value)} />
                <span className={"badge " + (proposal.date_was_extracted ? "existing" : "new")}>
                  {proposal.date_was_extracted ? "from note" : "today"}
                </span>
              </div>
              {proposal.description && <p className="desc">{proposal.description}</p>}
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

            <div className="review-body">
              <div className="pane">
                <label>Extracted data {dataParse.ok ? "" : "⚠︎ invalid JSON"}</label>
                <textarea
                  className={"json " + (dataParse.ok ? "" : "bad")}
                  value={dataText}
                  onChange={(e) => setDataText(e.target.value)}
                  spellCheck={false}
                />
              </div>
              <div className="pane preview">
                <label>Preview</label>
                <div className="preview-box">
                  {dataParse.ok ? <DataView value={dataParse.value} /> : <span className="muted">fix JSON to preview</span>}
                </div>
              </div>
            </div>

            <div className="review-actions">
              <button onClick={resetAll}>Discard</button>
              <button className="primary" onClick={onSave} disabled={!dataParse.ok}>
                Save to {catName.trim().toLowerCase() || "…"}
              </button>
            </div>
          </section>
        )}

        <section className="ask chat">
          {(messages.length > 0 || asking) && (
            <div className="chat-thread">
              {messages.map((m, i) => (
                <div className={"bubble " + m.role} key={i}>
                  <p>{m.content}</p>
                  {m.sources && m.sources.length > 0 && (
                    <div className="sources">
                      {m.sources.slice(0, 4).map((s) => (
                        <span className="source-chip" key={s.note_id} title={s.snippet}>
                          {s.category ?? "note"} · {s.event_date}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
              ))}
              {asking && !chatRecording && (
                <div className="bubble assistant">
                  <p className="muted">thinking…</p>
                </div>
              )}
            </div>
          )}
          <div className="ask-bar">
            <input
              value={askQ}
              placeholder="Ask about your day — “what was my last workout?”, “what did I do yesterday?”"
              onChange={(e) => setAskQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") sendMessage(askQ);
              }}
              disabled={asking}
            />
            <button
              className={"ghost mic" + (chatRecording ? " recording" : "")}
              onClick={onChatMic}
              disabled={asking && !chatRecording}
              title="Ask by voice"
            >
              {dlModel ? "…" : chatRecording ? "⏹" : "🎙"}
            </button>
            <button className="primary" onClick={() => sendMessage(askQ)} disabled={asking || !askQ.trim()}>
              {asking ? "…" : "Ask"}
            </button>
          </div>
          <div className="chat-tools">
            <button className="link" onClick={toggleMute} title="Speak answers aloud">
              {muted ? "🔇 muted" : "🔊 voice on"}
            </button>
            {messages.length > 0 && (
              <button className="link" onClick={clearChat}>
                clear
              </button>
            )}
          </div>
        </section>

        <section className="timeline">
          <h2>Timeline</h2>
          {notes.length === 0 && <p className="muted">Nothing yet. File your first note above.</p>}
          {notes.map((n) => (
            <article className="note" key={n.id}>
              <div className="note-meta">
                <span className="note-cat">{n.category ?? "uncategorized"}</span>
                {n.source === "photo" && <span className="src-tag">📷 photo</span>}
                <time title={`logged ${new Date(n.created_at).toLocaleString()}`}>
                  {new Date(n.event_date + "T00:00:00").toLocaleDateString(undefined, {
                    weekday: "short",
                    month: "short",
                    day: "numeric",
                    year: "numeric",
                  })}
                </time>
              </div>
              {n.data && <DataView value={n.data} />}
              <details className="raw">
                <summary>raw note</summary>
                <pre>{n.raw_text}</pre>
              </details>
            </article>
          ))}
        </section>
        </>
        )}
      </main>

      <footer className="status">
        <span>
          {health
            ? `sqlite-vec ${health.vec_version} · ${health.models.length} models · ${
                health.models.some((m) => m.startsWith("qwen2.5vl")) ? "vision ready" : "no vision model"
              }`
            : "connecting to local models…"}
        </span>
        <button className="link" onClick={onBackup}>
          ⤓ Back up
        </button>
        {backupMsg && <span className="backup-msg">{backupMsg}</span>}
      </footer>

      {showPhone && <PhonePanel onClose={() => setShowPhone(false)} />}
    </div>
  );
}
