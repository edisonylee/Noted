import { useCallback, useEffect, useRef, useState } from "react";
import { Undo2 } from "lucide-react";
import {
  api,
  type TranscriptVocabularyPreview,
  type TranscriptVocabularyRule,
} from "./api";

const EMPTY_PREVIEW: TranscriptVocabularyPreview = {
  matching_segments: 0,
  occurrences: 0,
};

export function TranscriptVocabularySettings({ showHeading = true }: { showHeading?: boolean } = {}) {
  const [rules, setRules] = useState<TranscriptVocabularyRule[]>([]);
  const [heard, setHeard] = useState("");
  const [preferred, setPreferred] = useState("");
  const [preview, setPreview] = useState<TranscriptVocabularyPreview>(EMPTY_PREVIEW);
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const previewRequest = useRef(0);

  const loadRules = useCallback(async () => {
    setRules(await api.meetingTranscriptVocabularyList());
  }, []);

  useEffect(() => {
    loadRules().catch((reason) => setError(String(reason)));
  }, [loadRules]);

  useEffect(() => {
    const term = heard.trim();
    const requestId = ++previewRequest.current;
    if (!term) {
      setPreview(EMPTY_PREVIEW);
      return;
    }
    const timer = window.setTimeout(() => {
      api
        .meetingTranscriptVocabularyPreview(term)
        .then((next) => {
          if (requestId === previewRequest.current) setPreview(next);
        })
        .catch(() => {
          if (requestId === previewRequest.current) setPreview(EMPTY_PREVIEW);
        });
    }, 120);
    return () => window.clearTimeout(timer);
  }, [heard]);

  async function applyCorrection() {
    const transcribed = heard.trim();
    const spelling = preferred.trim();
    if (!transcribed || !spelling) return;
    setPending(true);
    setMessage(null);
    setError(null);
    try {
      const result = await api.meetingTranscriptVocabularyApply(transcribed, spelling);
      setMessage(
        result.changed_segments > 0
          ? `Corrected ${result.changed_segments} ${result.changed_segments === 1 ? "line" : "lines"} and saved this spelling for future transcripts.`
          : "Saved this spelling for future transcripts. No existing lines needed a change."
      );
      setHeard("");
      setPreferred("");
      await loadRules();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPending(false);
    }
  }

  async function undoCorrection(rule: TranscriptVocabularyRule) {
    if (rule.last_batch_id == null) return;
    setPending(true);
    setMessage(null);
    setError(null);
    try {
      const result = await api.meetingTranscriptVocabularyUndo(rule.last_batch_id);
      const skipped = result.skipped_segments
        ? ` ${result.skipped_segments} later-edited ${result.skipped_segments === 1 ? "line was" : "lines were"} left untouched.`
        : "";
      setMessage(
        `Restored ${result.restored_segments} ${result.restored_segments === 1 ? "line" : "lines"} and stopped the future correction.${skipped}`
      );
      await loadRules();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPending(false);
    }
  }

  async function removeRule(rule: TranscriptVocabularyRule) {
    setPending(true);
    setMessage(null);
    setError(null);
    try {
      await api.meetingTranscriptVocabularyRemove(rule.id);
      setMessage(`Stopped correcting “${rule.heard}” in future transcripts. Existing text was kept.`);
      await loadRules();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPending(false);
    }
  }

  const previewLabel = heard.trim()
    ? `${preview.occurrences} ${preview.occurrences === 1 ? "instance" : "instances"} across ${preview.matching_segments} ${preview.matching_segments === 1 ? "line" : "lines"}`
    : "Whole words and phrases; capitalization is ignored.";

  return (
    <div className="transcript-vocabulary-settings">
      {showHeading && (
        <div className="transcript-vocabulary-heading">
          <strong>Transcript corrections</strong>
          <span>Fix a recurring mishearing in saved text and every future transcript.</span>
        </div>
      )}
      <form
        className="vocabulary-editor"
        onSubmit={(event) => {
          event.preventDefault();
          void applyCorrection();
        }}
      >
        <label>
          <span>Transcribed as</span>
          <input
            value={heard}
            onChange={(event) => {
              setHeard(event.target.value);
              setMessage(null);
              setError(null);
            }}
            placeholder="BORROW"
            maxLength={120}
          />
        </label>
        <span className="vocabulary-connector">should be</span>
        <label>
          <span>Preferred spelling</span>
          <input
            value={preferred}
            onChange={(event) => {
              setPreferred(event.target.value);
              setMessage(null);
              setError(null);
            }}
            placeholder="BARO"
            maxLength={120}
          />
        </label>
        <button
          className="vocabulary-apply"
          type="submit"
          disabled={pending || !heard.trim() || !preferred.trim()}
        >
          {pending
            ? "Applying…"
            : preview.matching_segments > 0
              ? `Correct ${preview.matching_segments} ${preview.matching_segments === 1 ? "line" : "lines"}`
              : "Save spelling"}
        </button>
      </form>
      <div className="vocabulary-preview" aria-live="polite">{previewLabel}</div>
      {message && <p className="vocabulary-message" aria-live="polite">{message}</p>}
      {error && <p className="vocabulary-error">{error}</p>}
      {rules.length > 0 && (
        <div className="vocabulary-rules">
          <strong className="vocabulary-rules-title">Active corrections</strong>
          {rules.map((rule) => (
            <div className="vocabulary-rule" key={rule.id}>
              <div>
                <strong>{rule.heard}</strong>
                <span>should be</span>
                <strong>{rule.preferred}</strong>
              </div>
              <small>
                {rule.last_changed_segments != null
                  ? `${rule.last_changed_segments} ${rule.last_changed_segments === 1 ? "line" : "lines"} changed · future transcripts on`
                  : "Future transcripts on"}
              </small>
              <div className="vocabulary-rule-actions">
                {rule.last_batch_id != null && (
                  <button type="button" onClick={() => void undoCorrection(rule)} disabled={pending}>
                    <Undo2 size={13} /> Undo latest
                  </button>
                )}
                <button type="button" onClick={() => void removeRule(rule)} disabled={pending}>
                  Stop future correction
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
