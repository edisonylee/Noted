import { useState } from "react";
import { MessageSquare, Pencil, SmilePlus, Trash2 } from "lucide-react";
import { REACTIONS, REACTION_NAMES } from "../../services/team/reactions";
import { TeamAvatar } from "./TeamAvatar";
import { TeamDialog } from "./TeamDialog";
import { orgPath, team } from "./client";
import type { TeamChatMessage, TeamUser } from "./types";

export function MessageRow({
  org,
  user,
  message,
  person,
  canSend,
  onChanged,
  onReply,
  onEdit,
  onDelete,
  onProfile,
  showReplies = true,
  extras,
}: {
  org: string;
  user: string;
  message: TeamChatMessage;
  person: TeamUser;
  canSend: boolean;
  onChanged: (message: TeamChatMessage) => void;
  onReply: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onProfile: () => void;
  showReplies?: boolean;
  extras: boolean;
}) {
  const [picker, setPicker] = useState(false);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const date = new Date(message.created_at);
  const react = async (emoji: string, active: boolean) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const updated = await team.request<TeamChatMessage>(
        "PUT",
        orgPath(org, `/chat-messages/${message.id}/reactions`),
        { emoji, active },
      );
      onChanged(updated);
      setPicker(false);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <article
      className={`messages-message${message.author_id === user ? " own" : ""}`}
      aria-label={`Message from ${person.name}`}
    >
      <button
        className="team-avatar-button"
        aria-label={`View ${person.name}'s profile`}
        onClick={onProfile}
      >
        <TeamAvatar org={org} person={person} className="messages-avatar" />
      </button>
      <div className="messages-message-content">
        <header>
          <button className="message-author" onClick={onProfile}>
            {person.name}
          </button>
          {message.author_id === user && <small>you</small>}
          <time dateTime={message.created_at} title={date.toLocaleString()}>
            {date.toLocaleTimeString(undefined, {
              hour: "numeric",
              minute: "2-digit",
            })}
          </time>
          {message.edited_at && !message.deleted_at && <small>edited</small>}
        </header>
        {message.deleted_at ? (
          <p className="messages-deleted">Message deleted</p>
        ) : (
          <p>{message.body}</p>
        )}
        {!message.deleted_at && !!message.reactions?.length && (
          <div className="message-reactions" aria-label="Reactions">
            {message.reactions.map((reaction) => (
              <button
                key={reaction.emoji}
                className={reaction.reacted ? "reacted" : ""}
                aria-pressed={reaction.reacted}
                aria-label={`${REACTION_NAMES.get(reaction.emoji) ?? reaction.emoji}, ${reaction.count} ${reaction.count === 1 ? "reaction" : "reactions"}${reaction.reacted ? ", including you" : ""}`}
                title={`${reaction.names.join(", ")}${reaction.count > reaction.names.length ? ` and ${reaction.count - reaction.names.length} more` : ""}`}
                disabled={!canSend || busy}
                onClick={() => void react(reaction.emoji, !reaction.reacted)}
              >
                <span aria-hidden="true">{reaction.emoji}</span>
                <span>{reaction.count}</span>
              </button>
            ))}
            {canSend && (
              <button
                aria-label="Add another reaction"
                disabled={busy}
                onClick={() => {
                  setSearch("");
                  setPicker(true);
                }}
              >
                <SmilePlus size={14} />
              </button>
            )}
          </div>
        )}
        {showReplies && !!message.reply_count && (
          <button className="message-thread-link" onClick={onReply}>
            <MessageSquare size={14} />
            {message.reply_count}{" "}
            {message.reply_count === 1 ? "reply" : "replies"}
            <span>Open thread</span>
          </button>
        )}
        {error && !picker && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
      </div>
      <div className="messages-actions">
        {!message.deleted_at && canSend && extras && (
          <>
            <button
              className="team-text-button"
              aria-label={`React to message from ${person.name}`}
              title="Add reaction"
              disabled={busy}
              onClick={() => {
                setSearch("");
                setPicker(true);
              }}
            >
              <SmilePlus size={14} />
            </button>
            <button
              className="team-text-button"
              aria-label={`Reply to message from ${person.name}`}
              title="Reply in thread"
              onClick={onReply}
            >
              <MessageSquare size={14} />
            </button>
          </>
        )}
        {message.can_edit && canSend && (
          <button
            className="team-text-button"
            aria-label={`Edit message from ${person.name}`}
            title="Edit message"
            onClick={onEdit}
          >
            <Pencil size={14} />
          </button>
        )}
        {message.can_delete && (
          <button
            className="team-text-button"
            aria-label={`Delete message from ${person.name}`}
            title="Delete message"
            onClick={onDelete}
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>
      {picker && (
        <TeamDialog
          title="Add a reaction"
          busy={busy}
          onClose={() => setPicker(false)}
        >
          <div className="team-form">
            <label>
              Find an emoji
              <input
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Smile, thanks, celebrate…"
              />
            </label>
            {error && (
              <p role="alert" className="team-error">
                {error}
              </p>
            )}
            <div className="reaction-picker">
              {REACTIONS.filter(([emoji, name]) =>
                `${emoji} ${name}`.includes(search.toLowerCase().trim()),
              ).map(([emoji, name]) => {
                const selected =
                  message.reactions?.some(
                    (r) => r.emoji === emoji && r.reacted,
                  ) ?? false;
                return (
                  <button
                    key={emoji}
                    aria-label={name}
                    aria-pressed={selected}
                    disabled={busy}
                    title={name}
                    onClick={() => void react(emoji, !selected)}
                  >
                    {emoji}
                  </button>
                );
              })}
            </div>
            <p className="team-muted">
              Choose a reaction. Click one you’ve added to remove it.
            </p>
          </div>
        </TeamDialog>
      )}
    </article>
  );
}
