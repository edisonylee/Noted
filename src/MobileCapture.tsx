import { useEffect, useRef, useState } from "react";
import { ArrowUp, Camera, Check, Mic, Square } from "lucide-react";
import { api } from "./api";
import { startRecording, type Recorder } from "./audio";
import { fileToImg } from "./image";

// The phone's default screen: a fast, no-wait capture. Text/voice/photo go
// straight to the Mac via quickCapture; the Mac categorizes + files in the
// background. No review step — capture is instant.
export function MobileCapture({ onCaptured }: { onCaptured?: () => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const [voiceReady, setVoiceReady] = useState<boolean | null>(null);
  const [dlModel, setDlModel] = useState(false);
  const [saved, setSaved] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const recorderRef = useRef<Recorder | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.voiceStatus().then((s) => setVoiceReady(s.ready)).catch(() => setVoiceReady(false));
  }, []);

  function flashSaved() {
    setSaved(true);
    setTimeout(() => setSaved(false), 2600);
  }

  async function capture() {
    const t = text.trim();
    if (!t || busy) return;
    setBusy(true);
    setErr(null);
    try {
      await api.quickCapture(t);
      setText("");
      flashSaved();
      onCaptured?.();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function onPhoto(file?: File | null) {
    if (!file) return;
    setBusy(true);
    setErr(null);
    try {
      const img = await fileToImg(file);
      const path = await api.saveImage(img.base64, img.ext);
      await api.quickCapture("", "photo", path);
      flashSaved();
      onCaptured?.();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
      if (fileInput.current) fileInput.current.value = "";
    }
  }

  async function ensureVoice(): Promise<boolean> {
    if (voiceReady) return true;
    setDlModel(true);
    try {
      await api.downloadVoiceModel();
      setVoiceReady(true);
      return true;
    } catch (e) {
      setErr(`voice model download failed: ${e}`);
      return false;
    } finally {
      setDlModel(false);
    }
  }

  async function onMic() {
    setErr(null);
    if (!voiceReady) {
      await ensureVoice();
      return;
    }
    if (recording) {
      setRecording(false);
      setTranscribing(true);
      try {
        const { b64, sampleRate } = await recorderRef.current!.stop();
        const said = await api.transcribe(b64, sampleRate);
        if (said) setText((t) => (t ? t.trim() + " " : "") + said);
      } catch (e) {
        setErr(`transcription failed: ${e}`);
      } finally {
        setTranscribing(false);
        recorderRef.current = null;
      }
    } else {
      try {
        recorderRef.current = await startRecording();
        setRecording(true);
      } catch (e) {
        setErr(`couldn't access the microphone: ${e}`);
      }
    }
  }

  return (
    <div className="mobile-capture">
      <textarea
        value={text}
        placeholder="Brain-dump anything — your gym session, what you ate, who you saw. noted files it for you."
        onChange={(e) => setText(e.target.value)}
        disabled={busy}
        autoFocus
      />

      <input
        ref={fileInput}
        type="file"
        accept="image/*"
        capture="environment"
        hidden
        onChange={(e) => onPhoto(e.target.files?.[0])}
      />

      <div className="mc-actions">
        <button className="ghost" onClick={() => fileInput.current?.click()} disabled={busy}>
          <Camera size={18} /> Photo
        </button>
        <button
          className={"ghost" + (recording ? " recording" : "")}
          onClick={onMic}
          disabled={busy || transcribing || dlModel}
        >
          {recording ? <Square size={16} strokeWidth={2.5} /> : <Mic size={18} />}
          {dlModel ? "Downloading…" : transcribing ? "Transcribing…" : recording ? "Stop" : "Speak"}
        </button>
        <button className="mc-send primary" onClick={capture} disabled={busy || !text.trim()}>
          <ArrowUp size={20} strokeWidth={2.5} />
        </button>
      </div>

      {saved && (
        <div className="mc-saved">
          <Check size={16} /> Saved — noted will sort it out.
        </div>
      )}
      {err && <div className="mc-err">{err}</div>}
    </div>
  );
}
