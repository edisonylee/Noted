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
import "./message-attachments.css";

export type PendingAttachment = {
  id: string;
  name: string;
  size: number;
  data: string;
};
const limit = 5 * 1024 * 1024;
const sizeLabel = (size: number) =>
  size < 1024 * 1024
    ? `${Math.ceil(size / 1024)} KB`
    : `${(size / 1024 / 1024).toFixed(1)} MB`;
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
export function MessageAttachments({
  org,
  files,
}: {
  org: string;
  files: TeamAttachment[];
}) {
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  return (
    <div className="message-attachments">
      <ul aria-label="Attachments">
        {files.map((file) => (
          <li key={file.id}>
            <FileText size={18} />
            <span title={file.name}>
              {file.name}
              <small>{sizeLabel(file.size)}</small>
            </span>
            <button
              type="button"
              className="team-text-button"
              disabled={!!busy}
              aria-label={`Save ${file.name}`}
              onClick={async () => {
                setBusy(file.id);
                setError("");
                try {
                  if (isDesktop) await api.teamSaveAttachment(org, file.id);
                  else {
                    const result = await team.request<
                      TeamAttachment & { data: string }
                    >("GET", orgPath(org, `/attachments/${file.id}`));
                    const blob = new Blob(
                      [
                        Uint8Array.from(atob(result.data), (c) =>
                          c.charCodeAt(0),
                        ),
                      ],
                      { type: "application/octet-stream" },
                    );
                    const url = URL.createObjectURL(blob);
                    const a = document.createElement("a");
                    a.href = url;
                    a.download = result.name;
                    a.click();
                    setTimeout(() => URL.revokeObjectURL(url), 1000);
                  }
                } catch (e) {
                  setError(String(e));
                } finally {
                  setBusy("");
                }
              }}
            >
              {busy === file.id ? (
                <Loader size={14} className="spin" />
              ) : (
                <Download size={14} />
              )}{" "}
              Save
            </button>
          </li>
        ))}
      </ul>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
