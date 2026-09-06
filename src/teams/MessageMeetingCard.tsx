import { useEffect, useRef, useState } from "react";
import { BookOpen, ArrowUpRight } from "lucide-react";
import { team, orgPath } from "./client";
import "./message-collections.css";
type Source = {
  available: boolean;
  id?: string;
  title?: string;
  excerpt?: string;
  updated?: boolean;
};
export function MessageMeetingCard({
  org,
  message,
  onOpen,
}: {
  org: string;
  message: string;
  onOpen?: (id: string) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [source, setSource] = useState<Source | null>(null),
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
        const next = await team.request<Source>(
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
          <span>Meeting unavailable</span>
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
          Loading shared meeting…
        </div>
      );
    if (!source.available)
      return (
        <div className="message-source-card">
          Meeting unavailable or access removed
        </div>
      );
    return (
      <button
        className="message-source-card"
        onClick={() => source.id && onOpen?.(source.id)}
        disabled={!onOpen}
      >
        <BookOpen size={18} />
        <span>
          <strong>{source.title}</strong>
          {source.excerpt && (
            <span className="message-source-excerpt">{source.excerpt}</span>
          )}
          <small>
            {source.updated
              ? "Source updated · open current meeting"
              : "Shared meeting · open source"}
          </small>
        </span>
        <ArrowUpRight size={15} />
      </button>
    );
  };
  return <div ref={root}>{content()}</div>;
}
