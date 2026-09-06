import { useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Loader, Minus, Plus } from "lucide-react";
import {
  AnnotationMode,
  getDocument,
  GlobalWorkerOptions,
  type PDFDocumentProxy,
  type RenderTask,
} from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";

GlobalWorkerOptions.workerSrc = workerUrl;
// Resolve supporting fonts/decoders only from bundled assets, never document-provided URLs.
const assets = import.meta.glob<string>(
  "/node_modules/pdfjs-dist/{cmaps,standard_fonts,wasm}/*",
  { eager: true, query: "?url", import: "default" },
);
class BundledPdfData {
  async fetch({ kind, filename }: { kind: string; filename: string }) {
    const folder = {
      cMapUrl: "cmaps",
      standardFontDataUrl: "standard_fonts",
      wasmUrl: "wasm",
    }[kind];
    const url =
      folder && assets[`/node_modules/pdfjs-dist/${folder}/${filename}`];
    if (!url) throw new Error("PDF resource unavailable");
    const response = await fetch(url);
    if (!response.ok) throw new Error("PDF resource unavailable");
    return new Uint8Array(await response.arrayBuffer());
  }
}
export default function AttachmentPdfPreview({ bytes }: { bytes: Uint8Array }) {
  const [pdf, setPdf] = useState<PDFDocumentProxy | null>(null);
  const [page, setPage] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [width, setWidth] = useState(600);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [pageText, setPageText] = useState("");
  const container = useRef<HTMLDivElement>(null);
  const surface = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let alive = true;
    const task = getDocument({
      data: bytes.slice(),
      enableXfa: false,
      useWorkerFetch: false,
      BinaryDataFactory: BundledPdfData,
      maxImageSize: 16_000_000,
      canvasMaxAreaInBytes: 64_000_000,
      stopAtErrors: true,
    });
    task.promise
      .then((document) => {
        if (alive) {
          setPdf(document);
          setError("");
        }
      })
      .catch((error) => {
        if (alive) {
          setLoading(false);
          setError(
            error?.name === "PasswordException"
              ? "This PDF is password-protected. Save it to open in your PDF reader."
              : "This PDF could not be previewed. You can still save the original file.",
          );
        }
      });
    return () => {
      alive = false;
      void task.destroy();
    };
  }, [bytes]);
  useEffect(() => {
    const observer = new ResizeObserver((entries) =>
      setWidth(Math.max(160, entries[0].contentRect.width - 32)),
    );
    if (container.current) observer.observe(container.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!pdf) return;
    let alive = true;
    let render: RenderTask | undefined;
    const canvas = document.createElement("canvas");
    canvas.setAttribute("aria-hidden", "true");
    setLoading(true);
    setError("");
    setPageText("");
    const draw = async () => {
      try {
        const source = await pdf.getPage(page);
        if (!alive) return;
        const natural = source.getViewport({ scale: 1 });
        const scale = Math.min(width / natural.width, 1.5) * zoom;
        const viewport = source.getViewport({ scale });
        // Bound decoded canvas memory even for unusually large PDF page dimensions.
        const density = Math.min(
          window.devicePixelRatio || 1,
          2,
          Math.sqrt(8_000_000 / (viewport.width * viewport.height)),
        );
        canvas.width = Math.max(1, Math.floor(viewport.width * density));
        canvas.height = Math.max(1, Math.floor(viewport.height * density));
        canvas.style.width = `${viewport.width}px`;
        canvas.style.height = `${viewport.height}px`;
        render = source.render({
          canvas,
          viewport,
          transform: [density, 0, 0, density, 0, 0],
          annotationMode: AnnotationMode.DISABLE,
        });
        await render.promise;
        if (!alive) return;
        surface.current?.replaceChildren(canvas);
        setLoading(false);
        const content = await source.getTextContent();
        if (alive)
          setPageText(
            content.items
              .map((item) => ("str" in item ? item.str : ""))
              .join(" ")
              .slice(0, 200_000),
          );
      } catch (error) {
        if (alive) {
          setLoading(false);
          setError(
            "This page could not be rendered. Try another page or save the original file.",
          );
        }
      }
    };
    void draw();
    return () => {
      alive = false;
      render?.cancel();
      canvas.remove();
      canvas.width = canvas.height = 0;
    };
  }, [pdf, page, width, zoom]);
  return (
    <div className="attachment-pdf">
      <div className="attachment-preview-controls" aria-label="PDF controls">
        <button
          type="button"
          className="team-text-button"
          aria-label="Previous page"
          disabled={!pdf || page === 1}
          onClick={() => setPage((value) => value - 1)}
        >
          <ChevronLeft size={18} />
        </button>
        <span aria-live="polite">
          Page {page} of {pdf?.numPages ?? "…"}
        </span>
        <button
          type="button"
          className="team-text-button"
          aria-label="Next page"
          disabled={!pdf || page === pdf.numPages}
          onClick={() => setPage((value) => value + 1)}
        >
          <ChevronRight size={18} />
        </button>
        <div className="attachment-controls-divider" />
        <button
          type="button"
          className="team-text-button"
          aria-label="Zoom out"
          disabled={zoom <= 0.5}
          onClick={() => setZoom((value) => value - 0.25)}
        >
          <Minus size={16} />
        </button>
        <button
          type="button"
          className="team-text-button"
          aria-label="Fit PDF to width"
          onClick={() => setZoom(1)}
        >
          {Math.round(zoom * 100)}%
        </button>
        <button
          type="button"
          className="team-text-button"
          aria-label="Zoom in"
          disabled={zoom >= 2}
          onClick={() => setZoom((value) => value + 0.25)}
        >
          <Plus size={16} />
        </button>
      </div>
      <div
        ref={container}
        className="attachment-pdf-pages"
        tabIndex={0}
        aria-label={`PDF page ${page}`}
      >
        {loading && (
          <p className="attachment-preview-status" role="status">
            <Loader size={18} className="spin" /> Rendering page…
          </p>
        )}
        {error && (
          <p className="attachment-preview-status" role="alert">
            {error}
          </p>
        )}
        <div
          ref={surface}
          className="attachment-pdf-canvas"
          hidden={loading || !!error}
        />
        <div className="sr-only">{pageText}</div>
      </div>
    </div>
  );
}
