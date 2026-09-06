import { useEffect, useRef, useState } from "react";
import { ArrowUp, Check, Loader2, MessageCircle, Mic, Palette, Settings2, Square, Volume2, VolumeX, X } from "lucide-react";
import { api, isDesktop, type AskEntity, type AskSource, type BrainVaultStatus, type ChatProposal } from "./api";
import { colorForType } from "./entityColors";
import { applyProposal, proposalText } from "./chatActions";
import { startRecording, type Recorder } from "./audio";
import { DataView } from "./DataView";
import { isThemeRequest, proposeTheme } from "./themeRequests";
import { useTheme } from "./useTheme";
import { CompanionLauncher, CompanionSettings } from "./Companion";
import { useCompanion } from "./companionStore";
import { usePetViewport } from "./CompanionLauncher";
import { panelNearPet, PET_SIZES, restorePet, type PetState } from "./companionMotion";
import { publishPetActivity } from "./companionDesktop";

type Msg = {
  failed?: boolean;
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
  const { preferences, pet } = useCompanion();
  const viewport = usePetViewport();
  const [petPosition, setPetPosition] = useState(() => restorePet(preferences.position, preferences.side, viewport, PET_SIZES[preferences.size]));
  const [customizing, setCustomizing] = useState(false);
  const launcherRef = useRef<HTMLButtonElement>(null);
  const { themes, previewTheme, clearPreview, activateTheme } = useTheme();
  const open = openProp ?? false;
  const openRef = useRef(open);
  openRef.current = open;
  const setOpen = (o: boolean) => {
    openRef.current = o;
    onOpenChange?.(o);
    if (!o) {
      recorderRef.current?.cancel();
      recorderRef.current = null;
      setRecording(false);
      setCustomizing(false);
      requestAnimationFrame(() => launcherRef.current?.focus());
    }
  };
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [asking, setAsking] = useState(false);
  const [confirming, setConfirming] = useState<number | null>(null);
  const [muted, setMuted] = useState(false);
  const [recording, setRecording] = useState(false);
  const [voiceReady, setVoiceReady] = useState(false);
  // Ask scope: "all" (everything) or a brain vault name (answers from that brain only).
  const [scope, setScope] = useState("all");
  const [vaults, setVaults] = useState<BrainVaultStatus[]>([]);
  const recorderRef = useRef<Recorder | null>(null);
  const threadRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const lastMessage = messages[messages.length - 1];
  const petActivity: PetState = asking || confirming !== null ? "working" : recording ? "waiting"
    : lastMessage?.failed ? "failed" : messages.some(message => message.proposal && !message.resolved) ? "waiting"
    : open && lastMessage?.role === "assistant" ? "review" : "idle";
  useEffect(() => { if (variant === "floating") publishPetActivity(petActivity); }, [petActivity, variant]);

  useEffect(() => {
    api.voiceStatus().then((s) => setVoiceReady(s.ready)).catch(() => {});
    api.brainListVaults().then(setVaults).catch(() => {});
  }, []);
  useEffect(() => {
    threadRef.current?.scrollTo({ top: threadRef.current.scrollHeight, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  }, [messages, asking, customizing, open]);
  useEffect(() => {
    if (!open || customizing) return;
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open, customizing]);
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);
  useEffect(() => () => clearPreview(), [clearPreview]);
  useEffect(() => () => recorderRef.current?.cancel(), []);
  useEffect(() => {
    if (open) return;
    recorderRef.current?.cancel();
    recorderRef.current = null;
    setRecording(false);
    setCustomizing(false);
  }, [open]);
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
    const history = messages
      .filter((message) => !message.proposal)
      .map((m) => ({ role: m.role, content: m.content }));
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
      setMessages((p) => [...p, { role: "assistant", content: `Sorry — ${e}`, failed: true }]);
    } finally {
      setAsking(false);
    }
  }

  async function confirmProposal(i: number, p: ChatProposal) {
    if (confirming !== null) return;
    setConfirming(i);
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
      setMessages((ms) => [...ms, { role: "assistant", content: `Couldn’t apply that — ${e}`, failed: true }]);
    } finally {
      setConfirming(null);
    }
  }

  function cancelProposal(i: number) {
    if (messages[i]?.proposal?.action === "apply_theme") clearPreview();
    setMessages((ms) => ms.map((m, idx) => (idx === i ? { ...m, resolved: "cancelled" } : m)));
    setMessages((ms) => [...ms, { role: "assistant", content: "Okay, left it as is." }]);
  }

  async function onMic() {
    if (!(await ensureVoice()) || !openRef.current) return;
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
        const recorder = await startRecording();
        if (!openRef.current) { recorder.cancel(); return; }
        recorderRef.current = recorder;
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

  return (
    <>
    {variant === "floating" && <CompanionLauncher open={open} activity={petActivity} onClick={() => setOpen(!open)} onMove={setPetPosition} buttonRef={launcherRef} />}
    {open && (
    <div
      id={variant === "floating" ? "companion-chat" : undefined}
      style={variant === "floating" ? { ...panelNearPet(petPosition, viewport, PET_SIZES[preferences.size], Math.min(customizing ? 480 : 420, viewport.width - 24), customizing ? 680 : 570), right: "auto", bottom: "auto" } : undefined}
      className={"chat-panel" + (variant === "sheet" ? " chat-sheet" : ` companion-panel companion-panel-${preferences.side}${customizing ? " companion-customizing" : ""}`)}
      role="dialog"
      aria-label="Ask Noted"
    >
      <div className="chat-panel-head">
        {variant === "floating" ? <img className="companion-head-image" src={pet.image} alt="" /> : <MessageCircle className="chat-panel-icon" size={17} aria-hidden="true" />}
        <span className="title">
          {customizing ? "Your companion" : variant === "floating" ? preferences.name : "Ask Noted"}<span className="sub">{customizing ? "Make yourself at home" : "Your Noted assistant"}</span>
        </span>
        {!customizing && vaults.length > 0 && (
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
          {variant === "floating" && <button className="icon-btn" disabled={recording} onClick={() => setCustomizing(!customizing)} title={customizing ? "Back to chat" : "Customize companion"} aria-label={customizing ? "Back to chat" : "Customize companion"} aria-pressed={customizing}>
            {customizing ? <MessageCircle size={17} /> : <Settings2 size={17} />}
          </button>}
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

      {customizing ? <div className="companion-editor"><CompanionSettings /></div> : <>
      <div className="chat-thread" ref={threadRef} role="log" aria-live="polite">
        {messages.length === 0 && !asking && (
          <div className="chat-empty">
            <strong>{variant === "floating" ? `Hi, I’m ${preferences.name}.` : "What can I do for you?"}</strong>
            <span>Ask about your notes, or schedule a meeting in one sentence.</span>
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
                <button className="primary" disabled={confirming !== null} onClick={() => confirmProposal(i, m.proposal!)}>
                  {confirming === i ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
                  {confirming === i
                    ? m.proposal.action === "create_event" ? "Creating…" : "Applying…"
                    : "Confirm"}
                </button>
                <button className="ghost" disabled={confirming !== null} onClick={() => cancelProposal(i)}>
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
          aria-label="Message your assistant"
          placeholder="Ask or schedule something…"
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") send(input);
          }}
          disabled={asking}
        />
        <button className="send-btn" aria-label="Send message" onClick={() => send(input)} disabled={asking || !input.trim()}>
          <ArrowUp size={18} strokeWidth={2.5} />
        </button>
      </div>
      </>}
    </div>
    )}
    </>
  );
}
