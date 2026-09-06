import { useBackdropDismiss } from "./ui/useDismissal";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Bot, Check, ChevronDown, FileText, Loader2, ShieldAlert, X } from "lucide-react";
import { api, isDesktop, type AgentContextOptions, type AgentContextPreview, type AgentContextRequest } from "./api";
import { listen } from "./events";

function formatBytes(bytes: number): string {
  if (bytes < 1_000) return `${bytes} bytes`;
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(bytes < 10_000 ? 1 : 0)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

function candidateDate(value: string | null): string {
  if (!value) return "Date unavailable";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value.slice(0, 10)
    : date.toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}

export function AgentContextApproval() {
  const [requests, setRequests] = useState<AgentContextRequest[]>([]);
  const [meetingId, setMeetingId] = useState<number | null>(null);
  const [options, setOptions] = useState<AgentContextOptions>({
    include_summary: true,
    include_notes: true,
    include_transcript: true,
    max_bytes: 500_000,
  });
  const [preview, setPreview] = useState<AgentContextPreview | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const [busy, setBusy] = useState<"" | "approve" | "deny">("");
  const [error, setError] = useState<string | null>(null);
  const [previewOpen, setPreviewOpen] = useState(true);

  const refresh = useCallback(async () => {
    if (!isDesktop) return;
    try {
      setRequests(await api.agentContextPending());
    } catch {
      setRequests([]);
    }
  }, []);

  useEffect(() => {
    if (!isDesktop) return;
    void refresh();
    const subscription = listen("agent-context-requested", () => void refresh());
    return () => { void subscription.then((unlisten) => unlisten()); };
  }, [refresh]);

  const request = requests[0] ?? null;
  const candidate = useMemo(
    () => request?.candidates.find((value) => value.meeting_id === meetingId) ?? null,
    [meetingId, request],
  );

  useEffect(() => {
    if (!request) {
      setMeetingId(null);
      setPreview(null);
      return;
    }
    const first = request.candidates[0] ?? null;
    setMeetingId(first?.meeting_id ?? null);
    setOptions({
      include_summary: Boolean(first?.summary_available && request.requested.include_summary),
      include_notes: Boolean(first?.notes_available && request.requested.include_notes),
      include_transcript: Boolean(first && first.segment_count > 0 && request.requested.include_transcript),
      max_bytes: request.requested.max_bytes ?? 500_000,
    });
    setPreview(null);
    setError(null);
    setPreviewOpen(true);
  }, [request?.id]);

  useEffect(() => {
    if (!request || !candidate || meetingId == null) {
      setPreview(null);
      return;
    }
    if (!options.include_summary && !options.include_notes && !options.include_transcript) {
      setPreview(null);
      setError("Select at least one section to share.");
      return;
    }
    let cancelled = false;
    setPreviewBusy(true);
    setError(null);
    const timer = window.setTimeout(() => {
      api.agentContextPreview(request.id, meetingId, options)
        .then((value) => {
          if (!cancelled) setPreview(value);
        })
        .catch((reason) => {
          if (!cancelled) {
            setPreview(null);
            setError(String(reason));
          }
        })
        .finally(() => {
          if (!cancelled) setPreviewBusy(false);
        });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [candidate, meetingId, options.include_notes, options.include_summary, options.include_transcript, options.max_bytes, request]);

  async function resolve(decision: "approve" | "deny") {
    if (!request) return;
    setBusy(decision);
    setError(null);
    try {
      if (decision === "approve") {
        if (!preview || meetingId == null) return;
        await api.agentContextResolve({
          requestId: request.id,
          decision,
          meetingId,
          options,
          previewHash: preview.packet_hash,
        });
      } else {
        await api.agentContextResolve({ requestId: request.id, decision });
      }
      await refresh();
    } catch (reason) {
      setError(String(reason));
      if (decision === "approve" && meetingId != null) {
        try {
          setPreview(await api.agentContextPreview(request.id, meetingId, options));
        } catch {
          setPreview(null);
        }
      }
    } finally {
      setBusy("");
    }
  }

  // Cancelling a consent dialog can only deny; it must never grant access.
  const backdrop = useBackdropDismiss(() => { void resolve("deny"); }, !!busy);

  if (!isDesktop || !request) return null;

  return (
    <div className="modal-overlay agent-approval-overlay" role="presentation" {...backdrop}>
      <section className="agent-approval" role="dialog" aria-modal="true" aria-labelledby="agent-approval-title">
        <header className="agent-approval-head">
          <span className="agent-approval-icon"><Bot size={18} /></span>
          <div>
            <span className="agent-approval-kicker">Agent Context Pass</span>
            <h2 id="agent-approval-title">Share meeting context?</h2>
          </div>
          {requests.length > 1 && <span className="agent-approval-queue">1 of {requests.length}</span>}
        </header>

        <div className="agent-request-summary">
          <div>
            <span>Receiving client</span>
            <strong>{request.client_name}</strong>
            <small>
              Claimed identity{request.runtime_name ? ` · reports itself as ${request.runtime_name}` : ""}
            </small>
          </div>
          <div>
            <span>Declared purpose</span>
            <strong>{request.purpose}</strong>
            <small>Meeting lookup: “{request.query}”</small>
          </div>
        </div>

        {request.candidates.length === 0 ? (
          <div className="agent-no-match">
            <ShieldAlert size={17} />
            <span>
              <strong>No matching visible meeting</strong>
              <small>Deny this request and ask the client to use a more specific title, participant, or date.</small>
            </span>
          </div>
        ) : (
          <>
            <fieldset className="agent-candidates">
              <legend>Choose the exact meeting</legend>
              {request.candidates.map((value) => (
                <label key={value.meeting_id} className={meetingId === value.meeting_id ? "selected" : ""}>
                  <input
                    type="radio"
                    name="agent-meeting"
                    checked={meetingId === value.meeting_id}
                    onChange={() => {
                      setMeetingId(value.meeting_id);
                      setOptions((current) => ({
                        ...current,
                        include_summary: value.summary_available && current.include_summary,
                        include_notes: value.notes_available && current.include_notes,
                        include_transcript: value.segment_count > 0 && current.include_transcript,
                      }));
                    }}
                  />
                  <span>
                    <strong>{value.title}</strong>
                    <small>
                      {candidateDate(value.started_at)}
                      {value.attendees.length ? ` · ${value.attendees.join(", ")}` : ""}
                    </small>
                  </span>
                  {meetingId === value.meeting_id && <Check size={14} />}
                </label>
              ))}
            </fieldset>

            {candidate && (
              <fieldset className="agent-section-picker">
                <legend>Include in this one-time pass</legend>
                <label>
                  <input
                    type="checkbox"
                    checked={options.include_summary}
                    disabled={!candidate.summary_available}
                    onChange={(event) => setOptions((value) => ({ ...value, include_summary: event.target.checked }))}
                  />
                  <span><strong>Current summary</strong><small>{candidate.summary_available ? "Latest generated meeting notes" : "Not available"}</small></span>
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={options.include_notes}
                    disabled={!candidate.notes_available}
                    onChange={(event) => setOptions((value) => ({ ...value, include_notes: event.target.checked }))}
                  />
                  <span><strong>My notes</strong><small>{candidate.notes_available ? "Your verbatim notes" : "Not available"}</small></span>
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={options.include_transcript}
                    disabled={candidate.segment_count === 0}
                    onChange={(event) => setOptions((value) => ({ ...value, include_transcript: event.target.checked }))}
                  />
                  <span><strong>Full transcript</strong><small>{candidate.segment_count ? `${candidate.segment_count} timestamped lines` : "Not available"}</small></span>
                </label>
                <label className="agent-size-limit">
                  <span><strong>Maximum packet</strong><small>Exact content is never silently truncated</small></span>
                  <select
                    value={options.max_bytes ?? 500_000}
                    onChange={(event) => setOptions((value) => ({ ...value, max_bytes: Number(event.target.value) }))}
                  >
                    <option value={100_000}>100 KB</option>
                    <option value={500_000}>500 KB</option>
                    <option value={1_000_000}>1 MB</option>
                  </select>
                </label>
              </fieldset>
            )}

            <section className="agent-exact-preview">
              <button type="button" onClick={() => setPreviewOpen((open) => !open)} aria-expanded={previewOpen}>
                <span>
                  <FileText size={14} />
                  <strong>Exact content</strong>
                  {preview && <small>{formatBytes(preview.total_bytes)} · ~{preview.estimated_tokens.toLocaleString()} tokens</small>}
                </span>
                {previewBusy ? <Loader2 size={14} className="spin" /> : <ChevronDown size={14} className={previewOpen ? "open" : ""} />}
              </button>
              {previewOpen && (
                preview ? <pre>{preview.content}</pre> : <div className="agent-preview-empty">{previewBusy ? "Building exact preview…" : "Preview unavailable."}</div>
              )}
            </section>
          </>
        )}

        <div className="agent-disclosure-warning">
          <ShieldAlert size={15} />
          <p>
            Noted cannot verify which downstream model this client uses. Once delivered, revoking access or deleting the meeting cannot erase copies the client or model already received.
          </p>
        </div>

        {error && <div className="error" role="alert">{error}</div>}

        <footer className="agent-approval-actions">
          <button type="button" className="ghost-btn" onClick={() => void resolve("deny")} disabled={busy !== ""}>
            {busy === "deny" ? <Loader2 size={13} className="spin" /> : <X size={13} />} Deny
          </button>
          <button
            type="button"
            className="primary"
            onClick={() => void resolve("approve")}
            disabled={busy !== "" || !preview || previewBusy || request.candidates.length === 0}
          >
            {busy === "approve" ? <Loader2 size={13} className="spin" /> : <Check size={13} />} Approve once
          </button>
        </footer>
      </section>
    </div>
  );
}
