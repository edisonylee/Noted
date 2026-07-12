// The Ask page (Granola-style chat home): a hero composer with attach + voice,
// recipe shortcuts underneath, and a thread once the conversation starts.
// Same grounded local-model agent as before — answers cite sources, and any
// mutation (create event / category, edit an entry) is a confirm-first
// proposal. Replaces the old bottom-right floating assistant on desktop.

import { useEffect, useRef, useState } from "react";
import {
  ArrowUp,
  Check,
  ListTodo,
  Mic,
  Newspaper,
  Paperclip,
  AudioLines,
  CalendarPlus,
  Square,
  X,
} from "lucide-react";
import { api, type AskSource, type ChatProposal } from "./api";
import { applyProposal, proposalText } from "./chatActions";
import { startRecording, type Recorder } from "./audio";
import { fileToImg, type Img } from "./image";
import { DataView } from "./DataView";

type Msg = {
  role: "user" | "assistant";
  content: string;
  sources?: AskSource[];
  proposal?: ChatProposal;
  resolved?: "confirmed" | "cancelled";
  attachedImg?: string; // dataUrl thumbnail on user messages
};

// Recipes: saved prompts, one click away (Granola's recipes). `send: false`
// prefills the composer for prompts that need details filled in.
const RECIPES: { label: string; icon: "todo" | "recap" | "meetings" | "event"; prompt: string; send: boolean }[] = [
  {
    label: "List recent todos",
    icon: "todo",
    prompt: "List every open action item and todo from my recent notes and meetings, grouped by owner.",
    send: true,
  },
  {
    label: "Write weekly recap",
    icon: "recap",
    prompt:
      "Write a recap of my week from my notes of the last 7 days: key events, meetings and their outcomes, wins, and open threads.",
    send: true,
  },
  {
    label: "Summarize recent meetings",
    icon: "meetings",
    prompt: "Summarize my recent meetings — for each: the outcome, decisions made, and action items.",
    send: true,
  },
  {
    label: "Schedule a meeting",
    icon: "event",
    prompt: "Schedule a meeting called … on … at …",
    send: false,
  },
];

export function AskView({ onMutated }: { onMutated?: () => void }) {
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [recording, setRecording] = useState(false);
  const [img, setImg] = useState<Img | null>(null);
  const recorderRef = useRef<Recorder | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const threadRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, asking]);

  async function send(text: string) {
    const raw = text.trim();
    if ((!raw && !img) || asking) return;
    setInput("");
    setAsking(true);
    const attached = img;
    setImg(null);
    const history = messages
      .filter((m) => !m.proposal)
      .map((m) => ({ role: m.role, content: m.content }));
    setMessages((p) => [
      ...p,
      { role: "user", content: raw || "(photo)", attachedImg: attached?.dataUrl },
    ]);
    try {
      // A photo rides along as locally-OCR'd text — the chat model is text-only.
      let q = raw;
      if (attached) {
        const ocr = await api.ocrPhoto(attached.base64);
        q = `${raw || "Here's a photo — use its contents."}\n\n[text read from the attached photo]\n${ocr}`;
      }
      const res = await api.chat(q, history, undefined);
      if (res.kind === "proposal") {
        setMessages((p) => [
          ...p,
          { role: "assistant", content: proposalText(res.proposal), proposal: res.proposal },
        ]);
      } else {
        setMessages((p) => [...p, { role: "assistant", content: res.answer, sources: res.sources }]);
      }
    } catch (e) {
      setMessages((p) => [...p, { role: "assistant", content: `Sorry — ${e}` }]);
    } finally {
      setAsking(false);
    }
  }

  async function confirm(i: number, p: ChatProposal) {
    setMessages((ms) => ms.map((m, idx) => (idx === i ? { ...m, resolved: "confirmed" } : m)));
    try {
      const done = await applyProposal(p);
      setMessages((ms) => [...ms, { role: "assistant", content: done }]);
      onMutated?.();
    } catch (e) {
      setMessages((ms) => [...ms, { role: "assistant", content: `Couldn't apply that — ${e}` }]);
    }
  }

  function cancel(i: number) {
    setMessages((ms) => [
      ...ms.map((m, idx) => (idx === i ? { ...m, resolved: "cancelled" as const } : m)),
      { role: "assistant" as const, content: "Okay, left it as is." },
    ]);
  }

  async function onMic() {
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
        await api.downloadVoiceModel().catch(() => {});
        recorderRef.current = await startRecording();
        setRecording(true);
      } catch {
        /* mic denied */
      }
    }
  }

  async function pickFile(f?: File | null) {
    if (!f) return;
    try {
      setImg(await fileToImg(f));
    } catch {
      /* unreadable file */
    }
  }

  const recipeIcon = (k: (typeof RECIPES)[number]["icon"]) =>
    k === "todo" ? (
      <ListTodo size={13} />
    ) : k === "recap" ? (
      <Newspaper size={13} />
    ) : k === "meetings" ? (
      <AudioLines size={13} />
    ) : (
      <CalendarPlus size={13} />
    );

  const composer = (
    <div className="ask-composer">
      {img && (
        <div className="ask-attach-preview">
          <img src={img.dataUrl} alt="attached" />
          <button className="icon-btn" onClick={() => setImg(null)} aria-label="Remove photo">
            <X size={13} />
          </button>
        </div>
      )}
      <textarea
        ref={inputRef}
        rows={1}
        value={input}
        placeholder="Summarize my meetings this week"
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send(input);
          }
        }}
        disabled={asking}
        onPaste={(e) => {
          const f = Array.from(e.clipboardData.files)[0];
          if (f) {
            e.preventDefault();
            pickFile(f);
          }
        }}
      />
      <div className="ask-composer-row">
        <button
          className="icon-btn"
          onClick={() => fileRef.current?.click()}
          title="Attach a photo (read locally)"
        >
          <Paperclip size={15} />
        </button>
        <input
          ref={fileRef}
          type="file"
          accept="image/*"
          hidden
          onChange={(e) => {
            pickFile(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
        <span className="spacer" />
        <button
          className={"icon-btn ask-mic" + (recording ? " recording" : "")}
          onClick={onMic}
          title="Ask by voice"
        >
          {recording ? <Square size={14} strokeWidth={2.5} /> : <Mic size={15} />}
        </button>
        <button
          className="ask-send"
          onClick={() => send(input)}
          disabled={asking || (!input.trim() && !img)}
          aria-label="Send"
        >
          <ArrowUp size={16} strokeWidth={2.5} />
        </button>
      </div>
    </div>
  );

  if (messages.length === 0) {
    return (
      <div className="ask-view ask-home">
        <h1 className="ask-hero">Hi Edison, ask anything</h1>
        {composer}
        <div className="ask-recipes">
          <span className="ask-recipes-label">Recipes</span>
          <div className="ask-recipe-row">
            {RECIPES.map((r) => (
              <button
                key={r.label}
                className="ask-recipe"
                onClick={() => {
                  if (r.send) send(r.prompt);
                  else {
                    setInput(r.prompt);
                    inputRef.current?.focus();
                  }
                }}
              >
                {recipeIcon(r.icon)} {r.label}
              </button>
            ))}
          </div>
        </div>
        {asking && <p className="quiet-empty">thinking…</p>}
      </div>
    );
  }

  return (
    <div className="ask-view">
      <div className="ask-thread" ref={threadRef}>
        {messages.map((m, i) => (
          <div className={"ask-bubble " + m.role} key={i}>
            {m.attachedImg && <img className="ask-bubble-img" src={m.attachedImg} alt="attached" />}
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
            {m.proposal?.action === "edit_entry" && !m.resolved && (
              <div className="proposal-preview">
                <DataView value={m.proposal.data} />
              </div>
            )}
            {m.proposal && !m.resolved && (
              <div className="proposal-actions">
                <button className="primary" onClick={() => confirm(i, m.proposal!)}>
                  <Check size={14} /> Confirm
                </button>
                <button className="ghost" onClick={() => cancel(i)}>
                  Cancel
                </button>
              </div>
            )}
            {m.proposal && m.resolved && <span className="proposal-state">{m.resolved}</span>}
          </div>
        ))}
        {asking && !recording && (
          <div className="ask-bubble assistant">
            <p className="muted">thinking…</p>
          </div>
        )}
      </div>
      {composer}
    </div>
  );
}
