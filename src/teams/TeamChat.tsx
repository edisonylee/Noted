import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  ChevronRight,
  Copy,
  History,
  Loader,
  Plus,
  Trash2,
} from "lucide-react";
import { MdBlock } from "../MeetingMarkdownView";
import { copyTeamText, orgPath, team } from "./client";
import { TeamDialog } from "./TeamDialog";
import type {
  TeamConversation,
  TeamConversationRow,
  TeamSnapshot,
} from "./types";

export function TeamChat({
  org,
  data,
  space,
  folder,
  selected,
  scopeName,
  id,
  onConversation,
  onSource,
}: {
  org: string;
  data: TeamSnapshot;
  space: string;
  folder: string;
  selected: string[];
  scopeName: string;
  id: string | null;
  onConversation: (id: string | null) => void;
  onSource: (id: string) => void;
}) {
  const [conversation, setConversation] = useState<TeamConversation | null>(
    null,
  );
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(false);
  const [reload, setReload] = useState(0);
  const [error, setError] = useState(""),
    [empty, setEmpty] = useState("");
  const [saved, setSaved] = useState<string[]>([]),
    [saving, setSaving] = useState("");
  const [copied, setCopied] = useState("");
  const [history, setHistory] = useState<TeamConversationRow[] | null>(null);
  const [removing, setRemoving] = useState<TeamConversationRow | null>(null);
  const [nextOffset, setNextOffset] = useState<number | null>(null),
    [historyBusy, setHistoryBusy] = useState(false);
  const generation = useRef(0),
    historyGeneration = useRef(0);
  const input = useRef<HTMLTextAreaElement>(null);
  const scopeKey = JSON.stringify([space, folder, selected]);
  const current = conversation?.id === id ? conversation : null;

  useEffect(() => {
    const version = ++generation.current;
    setConversation(null);
    setError("");
    setEmpty("");
    setBusy(false);
    setLoading(!!id);
    if (!id) {
      setQuestion("");
      return;
    }
    let inFlight = false;
    const check = async () => {
      if (inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      try {
        const value = await team.request<TeamConversation>(
          "GET",
          orgPath(org, `/conversations/${id}`),
        );
        if (version === generation.current) {
          setConversation((previous) =>
            previous &&
            previous.id === value.id &&
            previous.revision > value.revision
              ? previous
              : value,
          );
          setError("");
        }
      } catch (e) {
        if (version === generation.current) {
          setConversation(null);
          setError(String(e));
        }
      } finally {
        inFlight = false;
        if (version === generation.current) setLoading(false);
      }
    };
    void check();
    const timer = window.setInterval(() => {
      void check();
    }, 30_000);
    window.addEventListener("focus", check);
    return () => {
      ++generation.current;
      clearInterval(timer);
      window.removeEventListener("focus", check);
    };
  }, [org, id, scopeKey, reload]);
  useEffect(
    () => () => {
      ++generation.current;
      ++historyGeneration.current;
    },
    [],
  );

  const ask = async (event?: FormEvent) => {
    event?.preventDefault();
    if (busy || !question.trim() || (id && !current)) return;
    const version = generation.current,
      submitted = question.trim();
    setBusy(true);
    setError("");
    setEmpty("");
    try {
      const result = await team.ask(
        org,
        current
          ? { question: submitted, conversation_id: current.id }
          : {
              question: submitted,
              space_id: space,
              folder_id: folder,
              note_ids: selected,
            },
      );
      if (version !== generation.current) return;
      if (result.conversation) {
        setConversation(result.conversation);
        onConversation(result.conversation.id);
      } else setEmpty(result.answer);
      setQuestion("");
      input.current?.focus();
    } catch (e) {
      if (version === generation.current) {
        setError(String(e));
        setConversation(null);
      }
    } finally {
      if (version === generation.current) setBusy(false);
    }
  };
  const loadHistory = async (offset = 0) => {
    const version = ++historyGeneration.current;
    setHistoryBusy(true);
    setError("");
    if (!offset) setHistory([]);
    try {
      const value = await team.request<{
        conversations: TeamConversationRow[];
        next_offset: number | null;
      }>("GET", orgPath(org, `/conversations?offset=${offset}`));
      if (version === historyGeneration.current) {
        setHistory((rows) =>
          offset
            ? [...(rows ?? []), ...value.conversations]
            : value.conversations,
        );
        setNextOffset(value.next_offset);
      }
    } catch (e) {
      if (version === historyGeneration.current) setError(String(e));
    } finally {
      if (version === historyGeneration.current) setHistoryBusy(false);
    }
  };
  const closeHistory = () => {
    ++historyGeneration.current;
    setHistory(null);
    setRemoving(null);
    setHistoryBusy(false);
  };
  const activeScope = current?.scope;
  const label = activeScope
    ? activeScope.note_ids.length
      ? `${activeScope.note_ids.length} selected ${activeScope.note_ids.length === 1 ? "meeting" : "meetings"}`
      : (data.folders.find((f) => f.id === activeScope.folder_id)?.name ??
        data.spaces.find((s) => s.id === activeScope.space_id)?.name ??
        "All shared meetings")
    : selected.length
      ? `${selected.length} selected ${selected.length === 1 ? "meeting" : "meetings"}`
      : scopeName;
  return (
    <section className="team-ask" aria-label="Ask shared meetings">
      <div className="team-chat-tools">
        <span className="team-muted">
          {current ? "Your conversation" : "Ask your team’s meetings"}
        </span>
        <button
          className="team-text-button"
          onClick={() => {
            void loadHistory();
          }}
        >
          <History size={14} /> Chat history
        </button>
        {id && (
          <button
            className="team-text-button"
            onClick={() => {
              onConversation(null);
              input.current?.focus();
            }}
          >
            <Plus size={14} /> New conversation
          </button>
        )}
      </div>
      {error && !history && (
        <p className="team-error" role="alert">
          {error}
          {id && !current && (
            <button
              className="team-text-button"
              onClick={() => setReload((value) => value + 1)}
            >
              Recheck access
            </button>
          )}
        </p>
      )}
      {loading && (
        <p className="team-muted" role="status">
          Opening conversation…
        </p>
      )}
      {current?.turns.map((turn) => (
        <article className="team-chat-turn" key={turn.id}>
          <h2>{turn.question}</h2>
          <div className="team-answer">
            <MdBlock md={turn.answer} />
            <div className="team-chat-tools">
              <button
                className="team-text-button"
                disabled={!!saving || saved.includes(turn.id)}
                onClick={async () => {
                  const version = generation.current;
                  setSaving(turn.id);
                  try {
                    await team.request("POST", orgPath(org, "/answers"), turn);
                    if (version === generation.current)
                      setSaved((values) => [...values, turn.id]);
                  } catch (e) {
                    if (version === generation.current) setError(String(e));
                  } finally {
                    setSaving("");
                  }
                }}
              >
                {saved.includes(turn.id)
                  ? "Saved to your answers"
                  : saving === turn.id
                    ? "Saving…"
                    : "Save answer"}
              </button>
              <button
                className="team-text-button"
                onClick={async () => {
                  try {
                    await copyTeamText(
                      `${turn.question}\n\n${turn.answer}\n\n${turn.sources.map((s) => `[${s.citation}] ${s.title}`).join("\n")}`,
                    );
                    setCopied(turn.id);
                  } catch (e) {
                    setError(String(e));
                  }
                }}
              >
                <Copy size={13} />{" "}
                {copied === turn.id ? "Copied" : "Copy answer"}
              </button>
            </div>
            <div className="team-sources">
              {turn.sources.map((source) => (
                <button key={source.id} onClick={() => onSource(source.id)}>
                  <span>[{source.citation}]</span>
                  {source.title}
                </button>
              ))}
            </div>
            {turn.limited && (
              <p className="team-muted">
                This answer uses a selection of source excerpts. Narrow the
                scope for a closer review.
              </p>
            )}
          </div>
        </article>
      ))}
      {empty && (
        <p className="team-empty" role="status">
          {empty}
        </p>
      )}
      <form onSubmit={ask}>
        <label className="sr-only" htmlFor="team-question">
          {current ? "Ask a follow-up" : "Ask shared meetings"}
        </label>
        <textarea
          ref={input}
          id="team-question"
          rows={2}
          placeholder={
            current
              ? "Ask a follow-up…"
              : `What would you like to know about ${scopeName.toLowerCase()}?`
          }
          value={question}
          maxLength={6000}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void ask();
            }
          }}
        />
        <div className="team-ask-footer">
          <span>{label}</span>
          <button
            className="team-primary"
            disabled={
              busy ||
              loading ||
              !question.trim() ||
              (!!id && !current) ||
              (current?.revision ?? 0) >= 20
            }
          >
            {busy && <Loader size={15} className="spin" />}
            {busy ? "Reading sources…" : current ? "Ask follow-up" : "Ask"}
          </button>
        </div>
      </form>
      {(current?.revision ?? 0) >= 20 ? (
        <p className="team-muted">
          This conversation has 20 answers. Start a new conversation to keep
          exploring.
        </p>
      ) : (
        <p className="team-chat-privacy">
          Chats are saved privately to your account. Follow-ups use up to six
          recent turns and current meeting sources.
        </p>
      )}
      {!current && (
        <div className="team-recipe-shortcuts">
          {data.recipes
            .filter((r) => r.kind === "recipe")
            .slice(0, 4)
            .map((recipe) => (
              <button
                key={recipe.id}
                onClick={() => {
                  setQuestion(recipe.prompt);
                  input.current?.focus();
                }}
              >
                {recipe.name}
                <ChevronRight size={12} />
              </button>
            ))}
        </div>
      )}
      {history && (
        <TeamDialog
          title={removing ? "Delete conversation?" : "Your chat history"}
          onClose={closeHistory}
          busy={historyBusy}
        >
          {error && (
            <p className="team-error" role="alert">
              {error}
            </p>
          )}
          {removing ? (
            <div className="team-form">
              <p>
                Delete “{removing.question}” from your private chat history?
                Shared meetings and separately saved answers are kept.
              </p>
              <div className="team-chat-tools">
                <button
                  className="team-text-button"
                  disabled={historyBusy}
                  onClick={() => setRemoving(null)}
                  autoFocus
                >
                  Keep conversation
                </button>
                <button
                  className="team-primary"
                  disabled={historyBusy}
                  onClick={async () => {
                    const target = removing.id;
                    setHistoryBusy(true);
                    try {
                      await team.request(
                        "DELETE",
                        orgPath(org, `/conversations/${target}`),
                      );
                      if (target === id) onConversation(null);
                      setRemoving(null);
                      await loadHistory();
                    } catch (e) {
                      setError(String(e));
                      setHistoryBusy(false);
                    }
                  }}
                >
                  {historyBusy ? "Deleting…" : "Delete conversation"}
                </button>
              </div>
            </div>
          ) : (
            <>
              <p className="team-muted">
                Only your account can open these conversations. Source access is
                checked again each time.
              </p>
              <div className="team-note-list">
                {history.map((row) => (
                  <div key={row.id} className="team-note-row">
                    <button
                      disabled={!row.available || historyBusy}
                      onClick={() => {
                        onConversation(row.id);
                        closeHistory();
                      }}
                    >
                      <strong>{row.question}</strong>
                      <span className="team-note-meta">
                        {new Date(row.updated_at).toLocaleString()}
                      </span>
                    </button>
                    <button
                      className="team-text-button"
                      disabled={historyBusy}
                      aria-label={`Delete conversation: ${row.question}`}
                      onClick={() => {
                        setRemoving(row);
                        setError("");
                      }}
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                ))}
              </div>
              {!history.length && (
                <p className="team-empty">
                  {historyBusy
                    ? "Loading conversations…"
                    : "Your conversations will appear here after you ask a question."}
                </p>
              )}
              {nextOffset != null && (
                <button
                  className="team-text-button"
                  disabled={historyBusy}
                  onClick={() => {
                    void loadHistory(nextOffset);
                  }}
                >
                  Load more conversations
                </button>
              )}
            </>
          )}
        </TeamDialog>
      )}
    </section>
  );
}
