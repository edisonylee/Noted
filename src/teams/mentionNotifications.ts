import type { TeamChatRoom } from "./types";

export const conversationNotificationEvent = "noted:conversation-notifications-changed";
const preferenceKey = "noted:team-mention-notifications";
export const notificationPreferenceEvent = "noted:team-notifications-changed";
export function mentionNotificationsEnabled() {
  try { return localStorage.getItem(preferenceKey) === "true"; }
  catch { return false; }
}
export function setMentionNotificationsEnabled(enabled: boolean) {
  localStorage.setItem(preferenceKey, String(enabled));
  window.dispatchEvent(new Event(notificationPreferenceEvent));
}

export type MessageAlertMode = "messages" | "mentions";
export function messageAlertMode(): MessageAlertMode {
  try { return localStorage.getItem("noted:team-alert-mode") === "mentions" ? "mentions" : "messages"; }
  catch { return "messages"; }
}
export function setMessageAlertMode(mode: MessageAlertMode) {
  localStorage.setItem("noted:team-alert-mode", mode);
  window.dispatchEvent(new Event(notificationPreferenceEvent));
}

export function conversationAlertMode(room: TeamChatRoom, fallback: MessageAlertMode = messageAlertMode()): MessageAlertMode | "none" {
  return room.notification_mode && room.notification_mode !== "default" ? room.notification_mode : fallback;
}

let viewedRoom: string | null = null;
export function setViewedMessageRoom(room: string | null) { viewedRoom = room; }
export function isViewingMessageRoom(room: string) {
  return viewedRoom === room && document.visibilityState === "visible" && document.hasFocus();
}

// Baseline each room on first sight. A read marker or a message edit must not
// replay older mentions. Advance even when alerts are suppressed in the UI.
export class MentionNotificationTracker {
  private cursors = new Map<string, number>();
  update(scope: string, rooms: TeamChatRoom[], mode: MessageAlertMode | ((room: TeamChatRoom) => MessageAlertMode | "none") = "mentions") {
    const notify: TeamChatRoom[] = [];
    for (const room of rooms) {
      const key = `${scope}:${room.notification_user_id ?? ""}:${room.id}`;
      const previous = this.cursors.get(key);
      const cursor = room.notification_cursor;
      if (cursor == null) continue; // Compatible with older team servers.
      this.cursors.set(key, Math.max(previous ?? 0, cursor));
      const resolved = typeof mode === "function" ? mode(room) : mode;
      if (resolved === "none") continue;
      const latest = resolved === "messages" ? room.latest_unread_message_seq ?? room.latest_unread_mention_seq ?? 0 : room.latest_unread_mention_seq ?? 0;
      if (previous != null && !room.archived_at && latest > previous) notify.push(room);
    }
    return notify;
  }
}
