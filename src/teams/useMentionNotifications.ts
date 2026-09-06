import { sendTeamNotification } from "./desktopNotification";
import { useEffect } from "react";
import { isPermissionGranted } from "@tauri-apps/plugin-notification";
import { isDesktop } from "../api";
import { team, orgPath } from "./client";
import type { TeamChatRoom, TeamOrg } from "./types";
import { conversationAlertMode, conversationNotificationEvent, messageAlertMode, isViewingMessageRoom, mentionNotificationsEnabled, MentionNotificationTracker, notificationPreferenceEvent } from "./mentionNotifications";

export function useMentionNotifications() {
  useEffect(() => {
    if (!isDesktop) return;
    let stopped = false;
    let generation = 0;
    let timer: ReturnType<typeof setTimeout>;
    let tracker = new MentionNotificationTracker();
    const invalidate = () => { ++generation; };
    const reset = () => { ++generation; tracker = new MentionNotificationTracker(); };
    const poll = async () => {
      const epoch = generation;
      try {
        if (mentionNotificationsEnabled()) {
          const session = await team.status();
          if (!session.connected) reset();
          else {
            const orgs = await team.request<TeamOrg[]>("GET", "/v1/orgs");
            const permitted = await isPermissionGranted();
            for (const org of orgs) {
              if (stopped || epoch !== generation) break;
              let rooms: TeamChatRoom[];
              try {
                rooms = await team.request<TeamChatRoom[]>("GET", orgPath(org.id, "/chat-rooms"));
              } catch {
                continue; // One revoked workspace must not silence the others.
              }
              if (stopped || epoch !== generation) break;
              const fallback = messageAlertMode();
              const mentions = tracker.update(`${session.server}:${org.id}`, rooms, (room) => conversationAlertMode(room, fallback));
              for (const room of mentions) {
                if (stopped || epoch !== generation) break;
                const mode = conversationAlertMode(room, fallback);
                if (mode === "none") continue;
                if (!permitted || !mentionNotificationsEnabled() || isViewingMessageRoom(room.id)) continue;
                const message = mode === "mentions" ? room.latest_unread_mention_id : room.latest_unread_message_id;
                await sendTeamNotification({
                  target: message && room.notification_user_id ? { server: session.server, org: org.id, user: room.notification_user_id, message } : undefined,
                  title: mode === "mentions" ? `You were mentioned in ${org.name}` : `New message in ${org.name}`,
                  body: room.kind === "direct" ? "You have a new direct message." : `New ${mode === "mentions" ? "mention" : "message"} in ${room.is_default ? "Team chat" : `#${room.name}`}.`,
                });
              }
            }
          }
        }
      } catch {
        // A disconnected server or revoked membership is retried on the next poll.
      } finally {
        if (!stopped) timer = setTimeout(poll, 3_000);
      }
    };
    window.addEventListener(notificationPreferenceEvent, reset);
    window.addEventListener(conversationNotificationEvent, invalidate);
    void poll();
    return () => { stopped = true; clearTimeout(timer); window.removeEventListener(notificationPreferenceEvent, reset); window.removeEventListener(conversationNotificationEvent, invalidate); };
  }, []);
}
