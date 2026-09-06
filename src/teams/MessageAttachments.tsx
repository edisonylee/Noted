import { useRef, useState } from "react";
import { Download, FileText, Loader, Paperclip, X } from "lucide-react";
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
export function AttachmentPicker({
  files,
  onChange,
  disabled,
  onBusy,
}: {
  files: PendingAttachment[];
  onChange: (files: PendingAttachment[]) => void;
  disabled: boolean;
  onBusy: (busy: boolean) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
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
          if (busy || disabled || !chosen.length) return;
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
          setBusy(true);
          onBusy(true);
          try {
            const next = await Promise.all(
              chosen.map(async (file) => {
                if (
                  !file.size ||
                  !/\.(png|jpe?g|pdf|txt|md|csv)$/i.test(file.name)
                )
                  throw new Error(
                    "Choose a nonempty PNG, JPEG, PDF, or text file.",
                  );
                const data = await new Promise<string>((resolve, reject) => {
                  const reader = new FileReader();
                  reader.onerror = () =>
                    reject(
                      new Error(
                        "Could not read the file. Try choosing it again.",
                      ),
                    );
                  reader.onload = () =>
                    resolve(String(reader.result).split(",")[1]);
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
            onChange([...files, ...next]);
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
            onBusy(false);
          }
        }}
      />
      <button
        type="button"
        className="team-text-button"
        disabled={disabled || busy || files.length >= 3}
        onClick={() => input.current?.click()}
        title="PNG, JPEG, PDF, or text · 5 MiB total"
      >
        {busy ? <Loader size={14} className="spin" /> : <Paperclip size={14} />}{" "}
        Attach files
      </button>
      {files.length > 0 && (
        <ul aria-label="Files to send">
          {files.map((file) => (
            <li key={file.id}>
              <FileText size={15} />
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
}
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
