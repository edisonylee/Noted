import { openUrl } from "@tauri-apps/plugin-opener";
import { isDesktop } from "./api";

// Tauri webviews do not reliably hand target="_blank" links to the user's
// browser. Use the native opener on desktop while preserving normal new-tab
// behavior for the phone/web client.
export function openExternalUrl(url: string): void {
  if (isDesktop) {
    void openUrl(url).catch((error) => {
      console.error("[external-url] Could not open URL", { url, error });
    });
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}
