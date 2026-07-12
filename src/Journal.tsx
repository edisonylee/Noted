import { useEffect, useRef, useState } from "react";
import { BookOpen, Loader, Send, Sparkles } from "lucide-react";
import { api, type NoteRow } from "./api";
import { easternDay, easternHour } from "./day";

type Msg = {
  role: "you" | "journal";
  text: string;
  meta?: string; // "saved · 3 entities → knowledge graph"
};

const isJournal = (cat: string | null) => cat?.toLowerCase() === "journal";

// Time-adaptive opening prompt, mirroring the home greeting's tone.
function journalPrompt(): string {
  const h = easternHour();
  if (h < 11) return "What's on your mind this morning?";
  if (h < 17) return "How is today actually going?";
  return "How did today feel? What stuck with you?";
}

export function JournalView({
  notes,
  onSaved,
}: {
  notes: NoteRow[];
  onSaved: () => void | Promise<void>;
}) {
  // Seed the thread once from today's already-saved reflections (notes are
  // newest-first; reverse for reading order). Replies aren't persisted — the
  // durable artifacts are the notes and the knowledge graph they feed — so
  // past entries come back as your side of the conversation only.
  const [msgs, setMsgs] = useState<Msg[]>(() => {
    const today = easternDay();
    return notes
      .filter((n) => n.event_date === today && n.entries.some((e) => isJournal(e.category)))
      .map((n) => ({ role: "you" as const, text: n.raw_text ?? "", meta: "saved earlier today" }))
      .filter((m) => m.text.trim())
      .reverse();
  });
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [msgs, busy]);

  async function send() {
    const text = input.trim();
    if (!text || busy) return;
    setError(null);
    setBusy(true);
    setInput("");
    // History for the agent = the visible thread, oldest first.
    const history = msgs.map((m) => ({
      role: m.role === "journal" ? "assistant" : "user",
      content: m.text,
    }));
    setMsgs((ms) => [...ms, { role: "you", text }]);
    try {
      const r = await api.journalReflect(text, history);
      const meta =
        r.entity_count > 0
          ? `saved · ${r.entity_count} ${r.entity_count === 1 ? "entity" : "entities"} → knowledge graph`
          : "saved";
      setMsgs((ms) => {
        const next = [...ms];
        // Stamp the save receipt onto the reflection we just appended.
        for (let i = next.length - 1; i >= 0; i--) {
          if (next[i].role === "you" && !next[i].meta) {
            next[i] = { ...next[i], meta };
            break;
          }
        }
        if (r.reply) next.push({ role: "journal", text: r.reply });
        return next;
      });
      await onSaved(); // refresh notes so the rest of the app sees the entry
    } catch (e) {
      setError(String(e));
      setInput(text); // don't lose the reflection on a failed save
      setMsgs((ms) => ms.filter((m, i) => !(i === ms.length - 1 && m.role === "you" && !m.meta)));
    } finally {
      setBusy(false);
      boxRef.current?.focus();
    }
  }

  return (
    <div className="journal">
      <header className="journal-head">
        <div className="journal-eyebrow">Journal</div>
        <h1 className="journal-title">{journalPrompt()}</h1>
        <p className="journal-sub">
          Private reflections, kept on this Mac. People, places and feelings you mention are woven
          into your personal knowledge graph.
        </p>
      </header>

      <div className="journal-thread">
        {msgs.length === 0 && !busy && (
          <div className="journal-empty">
            <BookOpen size={26} className="journal-empty-icon" />
            <p>
              Nothing yet today. Write like you'd talk — a moment, a worry, something that went
              well — and the journal will reflect it back.
            </p>
          </div>
        )}
        {msgs.map((m, i) =>
          m.role === "you" ? (
            <div className="journal-msg you" key={i}>
              <div className="journal-bubble">{m.text}</div>
              {m.meta && (
                <div className="journal-meta">
                  <Sparkles size={11} /> {m.meta}
                </div>
              )}
            </div>
          ) : (
            <div className="journal-msg agent" key={i}>
              <div className="journal-bubble">{m.text}</div>
            </div>
          )
        )}
        {busy && (
          <div className="journal-msg agent">
            <div className="journal-bubble thinking">
              <Loader size={14} className="spin" /> reflecting…
            </div>
          </div>
        )}
        <div ref={endRef} />
      </div>

      {error && <div className="error journal-error">{error}</div>}

      <div className="journal-composer">
        <textarea
          ref={boxRef}
          value={input}
          placeholder="Write a reflection… (Enter to send, Shift+Enter for a new line)"
          disabled={busy}
          autoFocus
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <button
          className="journal-send"
          onClick={send}
          disabled={busy || !input.trim()}
          title="Reflect (Enter)"
          aria-label="Send reflection"
        >
          {busy ? <Loader size={16} className="spin" /> : <Send size={16} />}
        </button>
      </div>
    </div>
  );
}
