// The record-prompt popup — its own small always-on-top window, drawn by us
// (Granola-style) because native notifications can't hold two actions and a
// live accent. Solid left bar = calendar prompt; dashed = mic detection.
// Accepting a calendar prompt opens the call link too; the backend closes
// this window once recording starts (or on dismiss).

import { useEffect, useState } from "react";
import { Mic, Video } from "lucide-react";
import { listen } from "./events";
import { api, type PromptPayload } from "./api";
import { joinUrl } from "./joinUrl";
import { openExternalUrl } from "./openExternalUrl";
import { readFilingContext } from "./filingContext";

export function RecordPrompt() {
  const [p, setP] = useState<PromptPayload | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.meetingPromptPayload().then((v) => setP(v ?? null)).catch(() => {});
    const sub = listen<PromptPayload>("meeting-detected", (e) => {
      setP(e.payload);
      setError(null);
    });
    return () => {
      sub.then((un) => un());
    };
  }, []);

  if (!p) return null;

  const accept = async () => {
    setBusy(true);
    setError(null);
    try {
      if (p.kind === "calendar" && p.event?.meet_link) {
        openExternalUrl(joinUrl(p.event.meet_link, p.event.account));
      }
      await api.meetingStart({
        title: p.meetingTitle,
        eventId: p.event?.id ?? undefined,
        eventJson: p.event ?? undefined,
        sourceBundle: p.bundleId ?? undefined,
        filingContext: readFilingContext(),
        captureMode: p.kind === "calendar" && !p.event?.meet_link ? "in_person" : "online",
      });
      // Backend closes this window on start.
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const dismiss = () => {
    api.meetingDismissPrompt(p.bundleId ?? undefined).catch(() => {});
  };

  return (
    <div className={"record-prompt " + p.kind} data-tauri-drag-region>
      <div className="rp-body" data-tauri-drag-region>
        <div className="rp-text" data-tauri-drag-region>
          <span className="rp-kicker">
            {p.title}
            {p.app ? ` · ${p.app}` : ""}
          </span>
          <strong className="rp-title">{p.meetingTitle}</strong>
          {error && <span className="rp-error">{error}</span>}
        </div>
        {p.kind !== "status" && (
          <div className="rp-actions">
            <button className="rp-record" onClick={accept} disabled={busy}>
              {p.kind === "calendar" && p.event?.meet_link ? <Video size={14} /> : <Mic size={14} />}
              {busy ? "Starting…" : p.kind === "calendar" ? "Join & record" : "Record"}
            </button>
            <button className="rp-dismiss" onClick={dismiss} disabled={busy}>
              Not now
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
