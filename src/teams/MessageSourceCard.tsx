import { useEffect, useRef, useState } from "react";
import { BookOpen, FileText, ArrowUpRight } from "lucide-react";
import { team, orgPath } from "./client";
import type { TeamSourceReference } from "./types";
import "./message-collections.css";
// The message body never carries the source's title or excerpt: the card is
// derived per viewer from live permissions, so it is fetched here and reads
// "unavailable" the moment access is lost, whatever kind the source is.
export function MessageSourceCard({
  org,
  message,
  onOpen,
}: {
  org: string;
  message: string;
  onOpen?: (id: string) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [source, setSource] = useState<TeamSourceReference | null>(null),
    [error, setError] = useState(false),
    [retry, setRetry] = useState(0);
  useEffect(() => {
    let active = true,
      visible = false,
      pending = false;
    const load = async () => {
      if (!visible || pending) return;
      pending = true;
      try {
        const next = await team.request<TeamSourceReference>(
          "GET",
          orgPath(org, `/chat-messages/${message}/source`),
        );
        if (active) {
          setSource(next);
          setError(false);
        }
      } catch {
        if (active) {
          setSource(null);
          setError(true);
        }
      } finally {
        pending = false;
      }
    };
    const observer = new IntersectionObserver((entries) => {
      visible = entries.some((entry) => entry.isIntersecting);
      if (visible) {
        setSource(null);
        void load();
      }
    });
    if (root.current) observer.observe(root.current);
    const timer = setInterval(() => void load(), 10000);
    return () => {
      active = false;
      clearInterval(timer);
      observer.disconnect();
    };
  }, [org, message, retry]);
  const content = () => {
    if (error)
      return (
        <div className="message-source-card">
          <span>Shared source unavailable</span>
          <button
            className="team-text-button"
            onClick={() => setRetry((n) => n + 1)}
          >
            Retry
          </button>
        </div>
      );
    if (!source)
      return (
        <div className="message-source-card" role="status">
          Loading shared source…
        </div>
      );
    if (!source.available)
      return (
        <div className="message-source-card">
          Shared source unavailable or access removed
        </div>
      );
    // Older servers omit kind; a meeting card is the safe reading.
    const document = source.kind === "document";
    const Icon = document ? FileText : BookOpen;
    const noun = document ? "document" : "meeting";
    return (
      <button
        className="message-source-card"
        onClick={() => onOpen?.(source.id)}
        disabled={!onOpen}
      >
        <Icon size={18} />
        <span>
          <strong>{source.title}</strong>
          {source.excerpt && (
            <span className="message-source-excerpt">{source.excerpt}</span>
          )}
          <small>
            {source.updated
              ? `Updated since shared · open current ${noun}`
              : `Shared ${noun} · open source`}
          </small>
        </span>
        <ArrowUpRight size={15} />
      </button>
    );
  };
  return <div ref={root}>{content()}</div>;
}
