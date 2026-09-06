import { useCallback, useEffect, useState } from "react";
import { team, orgPath } from "./client";
import type { TeamAttachment } from "./types";
import { decodeAttachmentPreview } from "./attachmentPreviewData";

type Preview = { url: string; bytes: Uint8Array<ArrayBuffer>; scope: string };
/** No disk cache or public attachment URLs. Resources live only while displayed. */
export function useAttachmentPreview(
  org: string,
  file: TeamAttachment,
  enabled: boolean,
) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const retry = useCallback(() => setAttempt((value) => value + 1), []);
  const { id, mime, size, name } = file;
  useEffect(() => {
    let alive = true;
    let resource: Preview | null = null;
    let pending = false;
    setPreview(null);
    setError("");
    if (!enabled) return;
    const load = async () => {
      if (!enabled || pending || document.hidden) return;
      pending = true;
      try {
        const data = await team.request<TeamAttachment & { data: string }>(
          "GET",
          orgPath(org, `/attachments/${encodeURIComponent(id)}`),
        );
        if (!alive) return;
        const bytes = decodeAttachmentPreview({ id, mime, size, name }, data);
        // Attachment IDs are immutable; successful reauthorization keeps the current viewer stable.
        if (!resource) {
          resource = {
            bytes,
            scope: `${org}/${id}`,
            url: URL.createObjectURL(new Blob([bytes], { type: mime })),
          };
          setPreview(resource);
        }
        setError("");
      } catch (error) {
        if (!alive) return;
        if (resource) URL.revokeObjectURL(resource.url);
        resource = null;
        setPreview(null);
        setError(
          error instanceof Error
            ? error.message
            : "Preview unavailable. Try again.",
        );
      } finally {
        pending = false;
      }
    };
    void load();
    const visible = () => {
      if (!document.hidden) void load();
    };
    window.addEventListener("focus", visible);
    document.addEventListener("visibilitychange", visible);
    return () => {
      alive = false;
      if (resource) URL.revokeObjectURL(resource.url);
      window.removeEventListener("focus", visible);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [org, id, mime, size, name, enabled, attempt]);
  return {
    preview: enabled && preview?.scope === `${org}/${id}` ? preview : null,
    error,
    retry,
  };
}
