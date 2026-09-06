import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import { Download, FileText, Loader, X } from "lucide-react";
import { api, isDesktop } from "../api";
import { team, orgPath } from "./client";
import type { TeamAttachment } from "./types";
import { AttachmentPreview } from "./AttachmentPreview";
import { attachmentPreviewKind, sizeLabel } from "./attachmentPreviewData";
import { useAttachmentPreview } from "./useAttachmentPreview";
import "./message-attachments.css";

export type PendingAttachment = {
  id: string;
  name: string;
  size: number;
  data: string;
};
const limit = 5 * 1024 * 1024;
export type AttachmentPickerHandle = {
  addFiles: (files: File[]) => void;
  open: () => void;
};
export const AttachmentPicker = forwardRef<
  AttachmentPickerHandle,
  {
    files: PendingAttachment[];
    onChange: (files: PendingAttachment[]) => void;
    disabled: boolean;
    onBusy: (busy: boolean) => void;
  }
>(function AttachmentPicker({ files, onChange, disabled, onBusy }, ref) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const busyRef = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const addFiles = async (chosen: File[]) => {
    if (busyRef.current || disabled || !chosen.length) return;
    setError("");
    if (
      files.length + chosen.length > 3 ||
      files.reduce((n, f) => n + f.size, 0) +
        chosen.reduce((n, f) => n + f.size, 0) >
        limit
    ) {
      setError("Choose up to three files, totaling 5 MiB or less.");
      return;
    }
    busyRef.current = true;
    setBusy(true);
    onBusy(true);
    try {
      const next = await Promise.all(
        chosen.map(async (file) => {
          if (!file.size || !/\.(png|jpe?g|pdf|txt|md|csv)$/i.test(file.name))
            throw new Error("Choose a nonempty PNG, JPEG, PDF, or text file.");
          const data = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onerror = () =>
              reject(
                new Error("Could not read the file. Try choosing it again."),
              );
            reader.onload = () => resolve(String(reader.result).split(",")[1]);
            reader.readAsDataURL(file);
          });
          return {
            id: crypto.randomUUID(),
            name: file.name,
            size: file.size,
            data,
          };
        }),
      );
      if (mounted.current) onChange([...files, ...next]);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      busyRef.current = false;
      if (mounted.current) {
        setBusy(false);
        onBusy(false);
      }
    }
  };
  useImperativeHandle(ref, () => ({
    addFiles: (chosen) => {
      void addFiles(chosen);
    },
    open: () => input.current?.click(),
  }));
  return (
    <div className="message-attachment-picker">
      <input
        ref={input}
        type="file"
        accept=".png,.jpg,.jpeg,.pdf,.txt,.md,.csv"
        multiple
        hidden
        onChange={async (e) => {
          const chosen = [...(e.target.files ?? [])];
          e.target.value = "";
          void addFiles(chosen);
        }}
      />
      {files.length > 0 && (
        <ul aria-label="Files to send">
          {files.map((file) => (
            <li key={file.id}>
              {/\.(png|jpe?g)$/i.test(file.name) ? (
                <img
                  className="message-attachment-preview"
                  src={`data:image/${/\.png$/i.test(file.name) ? "png" : "jpeg"};base64,${file.data}`}
                  alt=""
                />
              ) : (
                <FileText size={15} />
              )}
              <span title={file.name}>
                {file.name}
                <small>{sizeLabel(file.size)}</small>
              </span>
              <button
                type="button"
                className="team-text-button"
                disabled={disabled || busy}
                aria-label={`Remove ${file.name}`}
                onClick={() => onChange(files.filter((f) => f.id !== file.id))}
              >
                <X size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {files.length > 0 && (
        <p className="team-muted">
          Files are kept for this session until sent. Closing the app discards
          them.
        </p>
      )}
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
});
function AttachmentDownload({
  org,
  file,
}: {
  org: string;
  file: TeamAttachment;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <div className="attachment-download">
      <button
        type="button"
        className="team-text-button"
        disabled={busy}
        aria-label={`Save ${file.name}`}
        onClick={async () => {
          setBusy(true);
          setError("");
          try {
            if (isDesktop) await api.teamSaveAttachment(org, file.id);
            else {
              const result = await team.request<
                TeamAttachment & { data: string }
              >(
                "GET",
                orgPath(org, `/attachments/${encodeURIComponent(file.id)}`),
              );
              const blob = new Blob(
                [Uint8Array.from(atob(result.data), (c) => c.charCodeAt(0))],
                { type: "application/octet-stream" },
              );
              const url = URL.createObjectURL(blob);
              const anchor = document.createElement("a");
              anchor.href = url;
              anchor.download = result.name;
              anchor.click();
              setTimeout(() => URL.revokeObjectURL(url), 1000);
            }
          } catch (error) {
            setError(
              error instanceof Error ? error.message : "Could not save file.",
            );
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? <Loader size={14} className="spin" /> : <Download size={14} />}{" "}
        Save
      </button>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
function AttachmentCard({ org, file }: { org: string; file: TeamAttachment }) {
  const card = useRef<HTMLLIElement>(null);
  const [visible, setVisible] = useState(false);
  const [opened, setOpened] = useState(false);
  const [imageError, setImageError] = useState(false);
  const kind = attachmentPreviewKind(file.mime);
  const { preview, error } = useAttachmentPreview(
    org,
    file,
    visible && kind === "image" && !opened,
  );
  useEffect(() => {
    const observer = new IntersectionObserver((entries) =>
      setVisible(entries[0].isIntersecting),
    );
    if (card.current) observer.observe(card.current);
    return () => observer.disconnect();
  }, []);
  return (
    <li
      ref={card}
      className={`attachment-card ${kind === "image" ? "attachment-card-image" : ""}`}
    >
      {kind === "image" && (
        <button
          type="button"
          className="attachment-inline-image"
          aria-label={`Open image ${file.name}`}
          onClick={() => setOpened(true)}
        >
          {preview && !imageError ? (
            <img
              src={preview.url}
              alt={file.name}
              decoding="async"
              onError={() => setImageError(true)}
            />
          ) : (
            <span>
              {error || imageError ? (
                "Image preview unavailable · Click to open"
              ) : (
                <>
                  <Loader size={18} className="spin" /> Loading image…
                </>
              )}
            </span>
          )}
        </button>
      )}
      <div className="attachment-card-details">
        <button
          type="button"
          className="attachment-open"
          aria-label={`Preview ${file.name}`}
          disabled={!kind}
          onClick={() => setOpened(true)}
        >
          <FileText size={18} />
          <span>
            <strong title={file.name}>{file.name}</strong>
            <small>
              {kind === "pdf" ? "PDF" : kind === "image" ? "Image" : "Text"} ·{" "}
              {sizeLabel(file.size)} · Preview
            </small>
          </span>
        </button>
        <AttachmentDownload org={org} file={file} />
      </div>
      {opened && (
        <AttachmentPreview
          org={org}
          file={file}
          onClose={() => setOpened(false)}
          download={<AttachmentDownload org={org} file={file} />}
        />
      )}
    </li>
  );
}
export function MessageAttachments({
  org,
  files,
}: {
  org: string;
  files: TeamAttachment[];
}) {
  return (
    <div className="message-attachments">
      <ul aria-label="Attachments">
        {files.map((file) => (
          <AttachmentCard key={file.id} org={org} file={file} />
        ))}
      </ul>
    </div>
  );
}
