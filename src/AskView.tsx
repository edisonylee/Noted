// The Ask page (Granola-style chat home): a hero composer with attach + voice,
// recipe shortcuts underneath, and a thread once the conversation starts.
// Same grounded local-model agent as before — answers cite sources, and any
// mutation (create event / category, edit an entry) is a confirm-first
// proposal. Replaces the old bottom-right floating assistant on desktop.
//
// Saving is prompt-driven: there is no Save button — starting the message with
// explicit save phrasing ("save …", "note that …", "log this …", "jot down …")
// routes the draft into the capture review flow instead of the model. The
// trigger is deterministic (code decides, not the model), so a question can
// never be silently swallowed into a note.

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
  Palette,
  Square,
  X,
} from "lucide-react";
import { api, type AskEntity, type AskSource, type ChatProposal } from "./api";
import { colorForType } from "./entityColors";
import { applyProposal, proposalText } from "./chatActions";
import { startRecording, type Recorder } from "./audio";
import { fileToImg, type Img } from "./image";
import { DataView } from "./DataView";
import { isThemeRequest, proposeTheme } from "./themeRequests";
import { useTheme } from "./useTheme";

type Msg = {
  role: "user" | "assistant";
  content: string;
  sources?: AskSource[];
  entities?: AskEntity[]; // knowledge-graph nodes that informed the answer
  proposal?: ChatProposal;
  resolved?: "confirmed" | "cancelled";
  attachedImg?: string; // dataUrl thumbnail on user messages
};

// Explicit save phrasing at the START of a message. Bare "save"/"jot" read as
// imperatives; "note"/"log" are everyday nouns, so they need a marker
// ("note that …", "log this …", "note: …") before they count. "save me …"
// ("save me some time and…") is excluded — that's a request, not a capture.
const SAVE_TRIGGERS: RegExp[] = [
  /^save\b(?!\s+me\b)(\s+(this|that|it|a\s+note|note|the\s+following))?\s*[:,]?\s*/i,
  /^(note|log)\s+(this|that|down)\b\s*[:,]?\s*/i,
  /^(note|log)\s*:\s*/i,
  /^jot\b(\s+(this|that|down))?\s*[:,]?\s*/i,
  /^add\s+a\s+note\b\s*(that\b)?\s*[:,]?\s*/i,
];

// Returns the text to file (trigger + polite lead-in stripped) when the
// message asks to save, else null. Deterministic on purpose.
export function saveIntent(raw: string): string | null {
  let s = raw.trim();
  const polite = /^(please|pls|hey|can\s+you|could\s+you|would\s+you)[,\s]+/i;
  for (let m = s.match(polite); m; m = s.match(polite)) s = s.slice(m[0].length);
  for (const t of SAVE_TRIGGERS) {
    const m = s.match(t);
    if (m) return s.slice(m[0].length).trim();
  }
  return null;
}

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

export function AskView({
  onMutated,
  onSaveNote,
  onOpenEntity,
}: {
  onMutated?: () => void;
  onSaveNote?: (draft: { text: string; img: Img | null }) => Promise<void>;
  onOpenEntity?: (id: number) => void; // jump into the knowledge graph
}) {
  const { themes, previewTheme, clearPreview, activateTheme } = useTheme();
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [filing, setFiling] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [recording, setRecording] = useState(false);
  const [img, setImg] = useState<Img | null>(null);
  const recorderRef = useRef<Recorder | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const threadRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, asking]);
  useEffect(() => () => clearPreview(), [clearPreview]);

  async function send(text: string) {
    const raw = text.trim();
    if ((!raw && !img) || asking || filing) return;
    // Explicit save phrasing files the draft instead of asking the model. A
    // bare trigger with nothing to save ("save?") falls through to chat.
    if (onSaveNote) {
      const toFile = saveIntent(raw);
      if (toFile !== null && (toFile || img)) {
        await fileNote(toFile);
        return;
      }
    }
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
      if (isThemeRequest(q)) {
        const proposal = await proposeTheme(q, themes);
        if (proposal.action === "apply_theme") previewTheme(proposal.theme_id);
        setMessages((p) => [
          ...p.map((message) => message.proposal?.action === "apply_theme" && !message.resolved
            ? { ...message, resolved: "cancelled" as const }
            : message),
          { role: "assistant", content: proposalText(proposal), proposal },
        ]);
        return;
      }
      const res = await api.chat(q, history, undefined);
      if (res.kind === "proposal") {
        setMessages((p) => [
          ...p,
          { role: "assistant", content: proposalText(res.proposal), proposal: res.proposal },
        ]);
      } else {
        setMessages((p) => [
          ...p,
          { role: "assistant", content: res.answer, sources: res.sources, entities: res.entities },
        ]);
      }
    } catch (e) {
      setMessages((p) => [...p, { role: "assistant", content: `Sorry — ${e}` }]);
    } finally {
      setAsking(false);
    }
  }

  async function confirm(i: number, p: ChatProposal) {
    try {
      let done: string;
      if (p.action === "apply_theme") {
        await api.themeActivate(p.theme_id);
        if (!activateTheme(p.theme_id, false)) throw new Error("That theme is no longer installed.");
        done = `Applied “${p.theme_name}”. You can change it anytime in Settings → Themes.`;
      } else {
        done = await applyProposal(p);
      }
      setMessages((ms) => [
        ...ms.map((m, idx) => (idx === i ? { ...m, resolved: "confirmed" as const } : m)),
        { role: "assistant", content: done },
      ]);
      onMutated?.();
    } catch (e) {
      setMessages((ms) => [...ms, { role: "assistant", content: `Couldn't apply that — ${e}` }]);
    }
  }

  function cancel(i: number) {
    if (messages[i]?.proposal?.action === "apply_theme") clearPreview();
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
        if (q) setInput((current) => (current.trim() ? `${current.trim()} ${q}` : q));
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

  // File `raw` (the message with its save trigger stripped) through the
  // capture review flow. The draft stays in the composer if filing fails.
  async function fileNote(raw: string) {
    if ((!raw && !img) || asking || filing || !onSaveNote) return;
    setFiling(true);
    setFileError(null);
    try {
      await onSaveNote({ text: raw, img });
      setInput("");
      setImg(null);
    } catch {
      setFileError("Couldn’t file that note. Your draft is still here.");
    } finally {
      setFiling(false);
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
        placeholder={onSaveNote ? "Ask anything — start with “save” to file a note" : "Ask anything"}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send(input);
          }
        }}
        disabled={asking || filing}
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
          disabled={filing}
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
          disabled={filing}
          title="Dictate"
        >
          {recording ? <Square size={14} strokeWidth={2.5} /> : <Mic size={15} />}
        </button>
        <button
          className="ask-send"
          onClick={() => send(input)}
          disabled={asking || filing || (!input.trim() && !img)}
          aria-label="Ask"
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
        {(asking || filing) && <p className="quiet-empty">{filing ? "filing…" : "thinking…"}</p>}
        {fileError && <p className="ask-file-error">{fileError}</p>}
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
            {m.entities && m.entities.length > 0 && (
              <div className="sources graph-refs">
                {m.entities.map((e) => (
                  <button
                    className="source-chip entity-chip"
                    key={e.id}
                    title={`Open ${e.name} in the knowledge graph`}
                    onClick={() => onOpenEntity?.(e.id)}
                  >
                    <span className="ldot" style={{ background: colorForType(e.type) }} />
                    {e.name}
                  </button>
                ))}
              </div>
            )}
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
            {m.proposal?.action === "apply_theme" && !m.resolved && (
              <div className="theme-proposal-preview">
                <Palette size={15} />
                <span>The whole app is showing this preview. Confirm to keep it, or cancel to roll back.</span>
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
        {(asking || filing) && !recording && (
          <div className="ask-bubble assistant">
            <p className="muted">{filing ? "filing…" : "thinking…"}</p>
          </div>
        )}
      </div>
      {composer}
      {fileError && <p className="ask-file-error">{fileError}</p>}
    </div>
  );
}
