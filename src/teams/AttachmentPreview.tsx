import { Component, lazy, Suspense, useMemo, useState } from "react";
import { Loader, Maximize2, Minus, Plus } from "lucide-react";
import { TeamDialog } from "./TeamDialog";
import { useAttachmentPreview } from "./useAttachmentPreview";
import {
  attachmentPreviewKind,
  textAttachmentPreview,
} from "./attachmentPreviewData";
import type { TeamAttachment } from "./types";
import type { ReactNode } from "react";
import "./attachment-preview.css";
const PdfPreview = lazy(() => import("./AttachmentPdfPreview"));

class PreviewBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? (
      <p className="attachment-preview-status" role="alert">
        The PDF viewer is unavailable. Save the original file to open it in
        another app.
      </p>
    ) : (
      this.props.children
    );
  }
}

function TextPreview({ bytes }: { bytes: Uint8Array }) {
  const value = useMemo(() => {
    try {
      return textAttachmentPreview(bytes);
    } catch {
      return null;
    }
  }, [bytes]);
  if (!value)
    return (
      <p className="attachment-preview-status" role="alert">
        This file is not readable UTF-8 text. Save it to open with another app.
      </p>
    );
  return (
    <div
      className="attachment-text-preview"
      tabIndex={0}
      aria-label="File contents"
    >
      {value.truncated && (
        <p className="attachment-text-notice">
          Showing the first 200,000 characters. Save the file to read it in
          full.
        </p>
      )}
      <pre>{value.text}</pre>
    </div>
  );
}
export function AttachmentPreview({
  org,
  file,
  onClose,
  download,
}: {
  org: string;
  file: TeamAttachment;
  onClose: () => void;
  download: ReactNode;
}) {
  const { preview, error, retry } = useAttachmentPreview(org, file, true);
  const kind = attachmentPreviewKind(file.mime);
  const [zoom, setZoom] = useState(1);
  const [imageError, setImageError] = useState(false);
  return (
    <TeamDialog
      title={file.name}
      onClose={onClose}
      className="attachment-preview-dialog"
    >
      <div className="attachment-preview-meta">
        <span>
          {kind === "image"
            ? "Image"
            : kind === "pdf"
              ? "PDF document"
              : "Text file"}{" "}
          · {Math.max(1, Math.ceil(file.size / 1024)).toLocaleString()} KB
        </span>
        {download}
      </div>
      {error ? (
        <div className="attachment-preview-status" role="alert">
          <p>{error}</p>
          <button type="button" className="team-text-button" onClick={retry}>
            Try again
          </button>
        </div>
      ) : !preview ? (
        <p className="attachment-preview-status" role="status">
          <Loader size={18} className="spin" /> Loading preview…
        </p>
      ) : (
        <>
          {kind === "image" && (
            <>
              <div
                className="attachment-preview-controls"
                aria-label="Image controls"
              >
                <button
                  className="team-text-button"
                  type="button"
                  aria-label="Zoom out"
                  disabled={zoom <= 1}
                  onClick={() => setZoom((value) => value - 0.5)}
                >
                  <Minus size={16} />
                </button>
                <button
                  className="team-text-button"
                  type="button"
                  aria-label="Fit image"
                  onClick={() => setZoom(1)}
                >
                  <Maximize2 size={16} />{" "}
                  {zoom === 1 ? "Fit" : `${Math.round(zoom * 100)}%`}
                </button>
                <button
                  className="team-text-button"
                  type="button"
                  aria-label="Zoom in"
                  disabled={zoom >= 3}
                  onClick={() => setZoom((value) => value + 0.5)}
                >
                  <Plus size={16} />
                </button>
              </div>
              <div
                className="attachment-image-preview"
                tabIndex={0}
                aria-label="Image preview"
              >
                {imageError ? (
                  <p role="alert">
                    This image could not be displayed. You can still save the
                    original.
                  </p>
                ) : (
                  <img
                    src={preview.url}
                    alt={file.name}
                    style={
                      zoom === 1
                        ? undefined
                        : {
                            maxWidth: "none",
                            maxHeight: "none",
                            width: `${zoom * 100}%`,
                          }
                    }
                    onError={() => setImageError(true)}
                  />
                )}
              </div>
            </>
          )}
          {kind === "pdf" && (
            <PreviewBoundary>
              <Suspense
                fallback={
                  <p className="attachment-preview-status" role="status">
                    Preparing PDF viewer…
                  </p>
                }
              >
                <PdfPreview bytes={preview.bytes} />
              </Suspense>
            </PreviewBoundary>
          )}
          {kind === "text" && <TextPreview bytes={preview.bytes} />}
        </>
      )}
    </TeamDialog>
  );
}
