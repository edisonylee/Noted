import { useEffect, useRef, useState } from "react";
import { ArrowUp, Mic, Sparkles, Square, Volume2, VolumeX, X } from "lucide-react";
import { api, type AskSource } from "./api";
import { startRecording, type Recorder } from "./audio";

type Msg = { role: "user" | "assistant"; content: string; sources?: AskSource[] };

export function FloatingChat() {
  const [open, setOpen] = useState(false);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [muted, setMuted] = useState(false);
  const [recording, setRecording] = useState(false);
  const [voiceReady, setVoiceReady] = useState(false);
  const recorderRef = useRef<Recorder | null>(null);
  const threadRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.voiceStatus().then((s) => setVoiceReady(s.ready)).catch(() => {});
  }, []);
  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, asking]);

  async function ensureVoice() {
    if (voiceReady) return true;
    try {
      await api.downloadVoiceModel();
      setVoiceReady(true);
      return true;
    } catch {
      return false;
    }
  }

  async function send(text: string) {
    const q = text.trim();
    if (!q || asking) return;
    setInput("");
    const history = messages.map((m) => ({ role: m.role, content: m.content }));
    setMessages((p) => [...p, { role: "user", content: q }]);
    setAsking(true);
    try {
      const res = await api.chat(q, history);
      setMessages((p) => [...p, { role: "assistant", content: res.answer, sources: res.sources }]);
      if (!muted) api.speak(res.answer).catch(() => {});
    } catch (e) {
      setMessages((p) => [...p, { role: "assistant", content: `Sorry — ${e}` }]);
    } finally {
      setAsking(false);
    }
  }

  async function onMic() {
    if (!(await ensureVoice())) return;
    if (recording) {
      setRecording(false);
      setAsking(true);
      try {
        const { b64, sampleRate } = await recorderRef.current!.stop();
        const q = await api.transcribe(b64, sampleRate);
        setAsking(false);
        if (q) await send(q);
      } catch {
        setAsking(false);
      } finally {
        recorderRef.current = null;
      }
    } else {
      try {
        recorderRef.current = await startRecording();
        setRecording(true);
      } catch {
        /* mic denied — silently ignore */
      }
    }
  }

  function toggleMute() {
    setMuted((m) => {
      if (!m) api.stopSpeaking().catch(() => {});
      return !m;
    });
  }

  if (!open) {
    return (
      <button className="fab" onClick={() => setOpen(true)} aria-label="Ask the assistant">
        <Sparkles size={22} strokeWidth={2} />
      </button>
    );
  }

  return (
    <div className="chat-panel">
      <div className="chat-panel-head">
        <span className="assistant-mark">
          <Sparkles size={15} strokeWidth={2} />
        </span>
        <span className="title">
          Assistant<span className="sub">your log, answered</span>
        </span>
        <div className="tools">
          <button className="icon-btn" onClick={toggleMute} title={muted ? "Unmute" : "Mute voice"}>
            {muted ? <VolumeX size={17} /> : <Volume2 size={17} />}
          </button>
          <button className="icon-btn" onClick={() => setOpen(false)} title="Close">
            <X size={17} />
          </button>
        </div>
      </div>

      <div className="chat-thread" ref={threadRef}>
        {messages.length === 0 && !asking && (
          <div className="chat-empty">
            Ask about anything you&rsquo;ve logged.
            <span className="ex">&ldquo;what was my last workout?&rdquo;</span>
            <span className="ex">&ldquo;what did I do yesterday?&rdquo;</span>
          </div>
        )}
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
        {asking && !recording && (
          <div className="bubble assistant">
            <p className="muted">thinking&hellip;</p>
          </div>
        )}
      </div>

      <div className="chat-input-row">
        <button
          className={"mic-btn" + (recording ? " recording" : "")}
          onClick={onMic}
          title="Ask by voice"
        >
          {recording ? <Square size={15} strokeWidth={2.5} /> : <Mic size={17} />}
        </button>
        <input
          value={input}
          placeholder="Ask about your day…"
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") send(input);
          }}
          disabled={asking}
        />
        <button className="send-btn" onClick={() => send(input)} disabled={asking || !input.trim()}>
          <ArrowUp size={18} strokeWidth={2.5} />
        </button>
      </div>
    </div>
  );
}
