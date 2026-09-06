import { api } from "../api";

// The native command uses Apple's UserNotifications API and returns actual
// authorization/submission errors, including disabled sound authorization.
export function sendTeamNotification(options: { title: string; body: string }) {
  return api.teamNotificationSend(options.title, options.body);
}
