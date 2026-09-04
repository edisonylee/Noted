import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ArrowLeft, ChevronRight, FileText, Loader } from "lucide-react";
import { api, type NoteRow } from "./api";
import { DocumentEditor } from "./editor/DocumentEditor";
import {
  documentFingerprint,
  documentPlainText,
  storedDocumentOrPlainText,
  type StructuredDocument,
} from "./editor/document";

type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

export function NoteDocumentEditor({
  note,
  workspaceLabel,
  itemLabel,
  metadata,
  placement,
  controls,
  onBack,
  onSaved,
  children,
}: {
  note: NoteRow;
  workspaceLabel: string;
  itemLabel: string;
  metadata: ReactNode;
  placement?: ReactNode;
  controls?: ReactNode;
  onBack: () => void;
  onSaved: (note: NoteRow) => void | Promise<void>;
  children?: ReactNode;
}) {
  const [title, setTitle] = useState(note.title);
  const [document, setDocument] = useState<StructuredDocument>(() =>
    storedDocumentOrPlainText(note.document_json, note.raw_text)
  );
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const titleRef = useRef(title);
  const documentRef = useRef(document);
  const revisionRef = useRef(0);
  const persistedRevisionRef = useRef(0);
  const saveTimerRef = useRef<number | null>(null);
  const saveInFlightRef = useRef<Promise<boolean> | null>(null);
  const flushRef = useRef<() => Promise<boolean>>(async () => true);
  const onSavedRef = useRef(onSaved);
  const mountedRef = useRef(true);

  titleRef.current = title;
  documentRef.current = document;
  onSavedRef.current = onSaved;

  function markDirty() {
    revisionRef.current += 1;
    setSaveError(null);
    setSaveState("dirty");
    if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = window.setTimeout(() => void flushRef.current(), 700);
  }

  async function flush(): Promise<boolean> {
    if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = null;

    if (saveInFlightRef.current) {
      const previousSaved = await saveInFlightRef.current;
      if (!previousSaved) return false;
    }

    while (persistedRevisionRef.current !== revisionRef.current) {
      const revision = revisionRef.current;
      const snapshot = documentRef.current;
      const snapshotTitle = titleRef.current;
      const rawText = documentPlainText(snapshot);
      const documentJson = JSON.stringify(snapshot);

      if (mountedRef.current) {
        setSaveError(null);
        setSaveState("saving");
      }

      const operation = (async () => {
        try {
          await api.updateNoteDocument(
            note.id,
            snapshotTitle,
            rawText,
            documentJson,
          );
          persistedRevisionRef.current = revision;
          const updated = {
            ...note,
            title: snapshotTitle.trim(),
            raw_text: rawText,
            document_json: documentJson,
            updated_at: new Date().toISOString(),
          };
          try {
            await onSavedRef.current(updated);
          } catch {
            // The note is already durable. A later library refresh can repair
            // stale list metadata without presenting the document as unsaved.
          }
          if (mountedRef.current && revisionRef.current === revision) {
            setSaveState("saved");
          }
          return true;
        } catch (error) {
          if (mountedRef.current) {
            setSaveError(String(error));
            setSaveState("error");
          }
          return false;
        }
      })();

      saveInFlightRef.current = operation;
      const saved = await operation;
      if (saveInFlightRef.current === operation) saveInFlightRef.current = null;
      if (!saved) return false;
    }
    return true;
  }
  flushRef.current = flush;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
      if (revisionRef.current !== persistedRevisionRef.current) {
        void flushRef.current();
      }
    };
  }, []);

  async function closeEditor() {
    if (!(await flushRef.current())) return;
    onBack();
  }

  return (
    <main className="note-document" aria-label={`${itemLabel} editor`}>
      <header className="note-document-head">
        <button
          className="note-document-back"
          type="button"
          onClick={() => void closeEditor()}
          aria-label={`Back to ${workspaceLabel}`}
          title={`Back to ${workspaceLabel}`}
        >
          <ArrowLeft size={17} aria-hidden="true" />
        </button>
        <div className="note-document-breadcrumb" aria-label="Current document">
          <FileText size={14} aria-hidden="true" />
          <span>{workspaceLabel}</span>
          <ChevronRight size={13} aria-hidden="true" />
          <strong>{title.trim() || "Untitled"}</strong>
        </div>
        <div className="note-document-head-actions">
          <span className={`note-document-save-state ${saveState}`} role="status" aria-live="polite">
            {saveState === "dirty" && "Unsaved"}
            {saveState === "saving" && <><Loader size={11} className="spin" /> Saving</>}
            {saveState === "saved" && "Saved"}
            {saveState === "error" && "Not saved"}
          </span>
          {controls}
        </div>
      </header>

      <DocumentEditor
        value={document}
        onChange={(next) => {
          if (documentFingerprint(next) === documentFingerprint(documentRef.current)) return;
          documentRef.current = next;
          setDocument(next);
          markDirty();
        }}
        placeholder="Start writing…"
        ariaLabel="Document content"
        variant="page"
        pageHeader={(
          <header className="note-document-page-head">
            <div className="note-document-meta">{metadata}</div>
            <input
              className="note-document-title"
              value={title}
              onChange={(event) => {
                titleRef.current = event.target.value;
                setTitle(event.target.value);
                markDirty();
              }}
              placeholder="Untitled"
              aria-label="Document title"
              autoFocus
            />
            {placement}
          </header>
        )}
      />
      {saveError && <div className="note-document-error" role="alert">{saveError}</div>}
      {children}
    </main>
  );
}
