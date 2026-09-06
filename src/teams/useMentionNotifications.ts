import { sendTeamNotification } from "./desktopNotification";
import { useEffect } from "react";
import { isPermissionGranted } from "@tauri-apps/plugin-notification";
import { isDesktop } from "../api";
import { team, orgPath } from "./client";
import type { TeamChatRoom, TeamOrg } from "./types";
import { messageAlertMode, isViewingMessageRoom, mentionNotificationsEnabled, MentionNotificationTracker, notificationPreferenceEvent } from "./mentionNotifications";

export function useMentionNotifications() {
  useEffect(() => {
    if (!isDesktop) return;
    let stopped = false;
    let generation = 0;
    let timer: ReturnType<typeof setTimeout>;
    let tracker = new MentionNotificationTracker();
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
              const mode = messageAlertMode();
              const mentions = tracker.update(`${session.server}:${org.id}`, rooms, mode);
              for (const room of mentions) {
                if (stopped || epoch !== generation) break;
                if (!permitted || !mentionNotificationsEnabled() || isViewingMessageRoom(room.id)) continue;
                await sendTeamNotification({
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
    void poll();
    return () => { stopped = true; clearTimeout(timer); window.removeEventListener(notificationPreferenceEvent, reset); };
  }, []);
}
