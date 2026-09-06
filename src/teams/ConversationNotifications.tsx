import { useState } from "react";
import { Bell, BellOff, Loader } from "lucide-react";
import { TeamDialog } from "./TeamDialog";
import { team, orgPath } from "./client";
import { conversationNotificationEvent, mentionNotificationsEnabled } from "./mentionNotifications";
import type { ConversationAlertMode, TeamChatRoom } from "./types";

export function ConversationNotifications({ org, room, onSaved }: {
  org: string; room: TeamChatRoom; onSaved: (room: TeamChatRoom) => void;
}) {
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<ConversationAlertMode>(room.notification_mode ?? "default");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <>
    <button className="team-text-button" title={room.notification_mode === "none" ? "Conversation muted" : "Conversation notifications"} aria-label="Conversation notifications" onClick={() => {
      setMode(room.notification_mode ?? "default"); setError(""); setOpen(true);
    }}>{room.notification_mode === "none" ? <BellOff size={17} /> : <Bell size={17} />}</button>
    {open && <TeamDialog title="Conversation notifications" busy={busy} onClose={() => setOpen(false)}>
      <form className="team-form" onSubmit={async (event) => {
        event.preventDefault();
        if (busy) return;
        setBusy(true); setError("");
        try {
          const next = await team.request<TeamChatRoom>("PUT", orgPath(org, `/chat-rooms/${room.id}/notifications`), { mode });
          onSaved(next);
          window.dispatchEvent(new Event(conversationNotificationEvent));
          setOpen(false);
        } catch (e) { setError(String(e)); }
        finally { setBusy(false); }
      }}>
        <p>Choose desktop banners and sounds for this conversation, including its threads. This preference applies only to your account and syncs across your devices.</p>
        <label>Notify me about
          <select value={mode} onChange={(event) => setMode(event.target.value as ConversationAlertMode)} disabled={busy || room.notification_mode == null}>
            <option value="default">Use global setting</option>
            <option value="messages">All messages</option>
            <option value="mentions">Only @mentions</option>
            <option value="none">No alerts (muted)</option>
          </select>
        </label>
        <p className="team-muted">Unread counts and your mentions inbox remain available when muted.</p>
        {!mentionNotificationsEnabled() && <p role="status">Desktop notifications are currently off in Settings → Notifications. Enable them there to receive alerts.</p>}
        {room.notification_mode == null && <p role="status">Update the team server to use conversation notification controls.</p>}
        {error && <p className="team-error" role="alert">{error}</p>}
        <button className="team-primary" disabled={busy || room.notification_mode == null}>{busy && <Loader size={14} className="spin" />} Save</button>
      </form>
    </TeamDialog>}
  </>;
}
