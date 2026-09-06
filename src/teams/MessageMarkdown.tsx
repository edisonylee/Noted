import {
  createContext,
  memo,
  useContext,
  useId,
  useRef,
  useState,
  type RefObject,
  type ReactNode,
} from "react";
import Markdown, { type ExtraProps } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import { openExternalUrl } from "../openExternalUrl";
import { chatMarkdownText, chatMarkdownUrl } from "./chatMarkdown";
import "./message-markdown.css";

function MarkdownImage({ src, alt }: { src: string; alt: string }) {
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(false);
  if (!src) return <span>{alt || "Image"}</span>;
  return (
    <span className="chat-markdown-image">
      {!loaded || error ? (
        <button
          type="button"
          className="team-text-button"
          onClick={() => {
            setError(false);
            setLoaded(true);
          }}
        >
          {error ? "Retry image" : `Load image from ${new URL(src).hostname}`}
          {alt ? ` · ${alt}` : ""}
        </button>
      ) : (
        <img
          src={src}
          alt={alt}
          loading="lazy"
          referrerPolicy="no-referrer"
          onError={() => setError(true)}
        />
      )}
    </span>
  );
}
const ReferenceRenderer = createContext<
  ((text: string) => ReactNode) | undefined
>(undefined);
function MarkdownSpan({
  node,
  children,
}: ExtraProps & { children?: ReactNode }) {
  const render = useContext(ReferenceRenderer);
  return node?.properties.dataChatText ? (
    <>{render ? render(String(children)) : children}</>
  ) : (
    <span>{children}</span>
  );
}
// Parse only changed message bodies. Mention context can update without reparsing history.
const MarkdownDocument = memo(function MarkdownDocument({
  body,
  prefix,
  root,
}: {
  body: string;
  prefix: string;
  root: RefObject<HTMLDivElement | null>;
}) {
  return (
    <Markdown
      remarkPlugins={[remarkGfm, remarkBreaks]}
      rehypePlugins={[[chatMarkdownText, { prefix }]]}
      remarkRehypeOptions={{ clobberPrefix: prefix }}
      urlTransform={(url, key) => chatMarkdownUrl(url, key === "src")}
      components={{
        span: MarkdownSpan,
        a: ({ href, children, node: _node, ...props }) =>
          href ? (
            <a
              {...props}
              href={href}
              title={href.startsWith("#") ? undefined : href}
              rel="noopener noreferrer"
              onClick={(event) => {
                event.preventDefault();
                if (href.startsWith("#")) {
                  const target = root.current?.querySelector<HTMLElement>(
                    `#${CSS.escape(href.slice(1))}`,
                  );
                  target?.scrollIntoView({ block: "nearest" });
                  if (target) {
                    target.tabIndex = -1;
                    target.focus({ preventScroll: true });
                  }
                } else openExternalUrl(href);
              }}
            >
              {children}
            </a>
          ) : (
            <span>{children}</span>
          ),
        img: ({ src, alt }) => (
          <MarkdownImage
            key={src}
            src={typeof src === "string" ? src : ""}
            alt={alt ?? ""}
          />
        ),
        input: ({ checked }) => (
          <input
            type="checkbox"
            checked={!!checked}
            disabled
            readOnly
            aria-label={checked ? "Completed task" : "Incomplete task"}
          />
        ),
        table: ({ children }) => (
          <div
            className="message-markdown-table"
            tabIndex={0}
            role="region"
            aria-label="Message table"
          >
            <table>{children}</table>
          </div>
        ),
      }}
    >
      {body}
    </Markdown>
  );
});
export function MessageMarkdown({
  body,
  renderText,
}: {
  body: string;
  renderText?: (value: string) => ReactNode;
}) {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  return (
    <div ref={root} className="message-markdown">
      <ReferenceRenderer.Provider value={renderText}>
        <MarkdownDocument body={body} prefix={`message-${id}-`} root={root} />
      </ReferenceRenderer.Provider>
    </div>
  );
}
