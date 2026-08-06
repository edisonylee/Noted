import { useEffect, useRef, useState } from "react";
import { ArrowUp, Check, MessageCircle, Mic, Palette, Square, Volume2, VolumeX, X } from "lucide-react";
import { api, isDesktop, type AskEntity, type AskSource, type BrainVaultStatus, type ChatProposal } from "./api";
import { colorForType } from "./entityColors";
import { applyProposal, proposalText } from "./chatActions";
import { startRecording, type Recorder } from "./audio";
import { DataView } from "./DataView";
import { isThemeRequest, proposeTheme } from "./themeRequests";
import { useTheme } from "./useTheme";

type Msg = {
  role: "user" | "assistant";
  content: string;
  sources?: AskSource[];
  entities?: AskEntity[]; // knowledge-graph nodes that informed the answer
  proposal?: ChatProposal; // when set, render Confirm/Cancel
  resolved?: "confirmed" | "cancelled";
};

export function FloatingChat({
  onMutated,
  open: openProp,
  onOpenChange,
  variant = "floating",
}: {
  onMutated?: () => void;
  open?: boolean; // controlled (mobile "Ask" tab); omit for the floating FAB
  onOpenChange?: (open: boolean) => void;
  variant?: "floating" | "sheet";
}) {
  const { themes, previewTheme, clearPreview, activateTheme } = useTheme();
  const open = openProp ?? false;
  const setOpen = (o: boolean) => onOpenChange?.(o);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [muted, setMuted] = useState(false);
  const [recording, setRecording] = useState(false);
  const [voiceReady, setVoiceReady] = useState(false);
  // Ask scope: "all" (everything) or a brain vault name (answers from that brain only).
  const [scope, setScope] = useState("all");
  const [vaults, setVaults] = useState<BrainVaultStatus[]>([]);
  const recorderRef = useRef<Recorder | null>(null);
  const threadRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.voiceStatus().then((s) => setVoiceReady(s.ready)).catch(() => {});
    api.brainListVaults().then(setVaults).catch(() => {});
  }, []);
  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, asking]);
  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);
  useEffect(() => () => clearPreview(), [clearPreview]);
  const wasOpenRef = useRef(open);
  useEffect(() => {
    if (wasOpenRef.current && !open && messages.some((message) => message.proposal?.action === "apply_theme" && !message.resolved)) {
      clearPreview();
      setMessages((current) => current.map((message) =>
        message.proposal?.action === "apply_theme" && !message.resolved
          ? { ...message, resolved: "cancelled" as const }
          : message
      ));
    }
    wasOpenRef.current = open;
  }, [open, messages, clearPreview]);

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
      const res = await api.chat(q, history, scope === "all" ? undefined : scope);
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
        if (!muted && isDesktop) api.speak(res.answer).catch(() => {});
      }
    } catch (e) {
      setMessages((p) => [...p, { role: "assistant", content: `Sorry — ${e}` }]);
    } finally {
      setAsking(false);
    }
  }

  async function confirmProposal(i: number, p: ChatProposal) {
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
      setMessages((ms) => [...ms, { role: "assistant", content: `Couldn’t apply that — ${e}` }]);
    }
  }

  function cancelProposal(i: number) {
    if (messages[i]?.proposal?.action === "apply_theme") clearPreview();
    setMessages((ms) => ms.map((m, idx) => (idx === i ? { ...m, resolved: "cancelled" } : m)));
    setMessages((ms) => [...ms, { role: "assistant", content: "Okay, left it as is." }]);
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

  if (!open) return null;

  return (
    <div
      className={"chat-panel" + (variant === "sheet" ? " chat-sheet" : "")}
      role="dialog"
      aria-label="Ask Noted"
    >
      <div className="chat-panel-head">
        <MessageCircle className="chat-panel-icon" size={17} aria-hidden="true" />
        <span className="title">
          Ask Noted<span className="sub">your notes and meetings</span>
        </span>
        {vaults.length > 0 && (
          <select
            className="chat-scope"
            value={scope}
            onChange={(e) => setScope(e.target.value)}
            title="Limit answers to one knowledge base"
          >
            <option value="all">All</option>
            {vaults.map((v) => (
              <option key={v.vault} value={v.vault}>
                {v.vault}
              </option>
            ))}
          </select>
        )}
        <div className="tools">
          {isDesktop && (
            <button className="icon-btn" onClick={toggleMute} title={muted ? "Unmute" : "Mute voice"}>
              {muted ? <VolumeX size={17} /> : <Volume2 size={17} />}
            </button>
          )}
          <button className="icon-btn" onClick={() => setOpen(false)} title="Close">
            <X size={17} />
          </button>
        </div>
      </div>

      <div className="chat-thread" ref={threadRef}>
        {messages.length === 0 && !asking && (
          <div className="chat-empty">
            <strong>Your memory is ready.</strong>
            <span>Ask about a meeting, person, decision, plan, or anything you&rsquo;ve saved.</span>
          </div>
        )}
        {messages.map((m, i) => (
          <div className={"bubble " + m.role} key={i}>
            <p>{m.content}</p>
            {m.entities && m.entities.length > 0 && (
              <div className="sources graph-refs">
                {m.entities.map((e) => (
                  <span className="source-chip entity-chip" key={e.id}>
                    <span className="ldot" style={{ background: colorForType(e.type) }} />
                    {e.name}
                  </span>
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
                <button className="primary" onClick={() => confirmProposal(i, m.proposal!)}>
                  <Check size={14} /> Confirm
                </button>
                <button className="ghost" onClick={() => cancelProposal(i)}>
                  Cancel
                </button>
              </div>
            )}
            {m.proposal && m.resolved && (
              <span className="proposal-state">{m.resolved}</span>
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
          ref={inputRef}
          value={input}
          placeholder="Ask your memory…"
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
