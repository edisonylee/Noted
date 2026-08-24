import { invoke } from "@tauri-apps/api/core";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";

export const MOBILE_OPEN_NOTE_EVENT = "noted:open-note";
export const MOBILE_DEEP_LINK_ERROR_EVENT = "noted:deep-link-error";

type ResolvedMobileDeepLink = {
  destination: "note";
  libraryId: string;
  recordId: string;
};

type DeepLinkErrorDetail = { message: string };

const pendingOpenLinks: ResolvedMobileDeepLink[] = [];
const pendingErrors: DeepLinkErrorDetail[] = [];
let consumerReady = false;

function dispatchOpenLink(link: ResolvedMobileDeepLink) {
  window.dispatchEvent(new CustomEvent(MOBILE_OPEN_NOTE_EVENT, { detail: link }));
}

function dispatchDeepLinkError(detail: DeepLinkErrorDetail) {
  window.dispatchEvent(new CustomEvent(MOBILE_DEEP_LINK_ERROR_EVENT, { detail }));
}

/**
 * Marks the React shell ready only after its event listeners are installed.
 * This keeps a cold-launch URL from racing the first concurrent React render.
 */
export function connectMobileDeepLinkConsumer(): () => void {
  consumerReady = true;
  for (const link of pendingOpenLinks.splice(0)) dispatchOpenLink(link);
  for (const detail of pendingErrors.splice(0)) dispatchDeepLinkError(detail);
  return () => {
    consumerReady = false;
  };
}

function reportDeepLinkError(reason: unknown) {
  const message = reason instanceof Error ? reason.message : String(reason);
  const detail = { message };
  if (consumerReady) dispatchDeepLinkError(detail);
  else pendingErrors.push(detail);
}

export function reportMobileDeepLinkStartupError(reason: unknown) {
  reportDeepLinkError(reason);
}

async function openMobileDeepLink(url: string) {
  try {
    const link = await invoke<ResolvedMobileDeepLink>("resolve_mobile_deep_link", { url });
    if (link.destination !== "note") throw new Error("Unsupported Noted link destination");
    const detail = {
      destination: "note" as const,
      libraryId: link.libraryId,
      recordId: link.recordId,
    };
    if (consumerReady) dispatchOpenLink(detail);
    else pendingOpenLinks.push(detail);
  } catch (reason) {
    reportDeepLinkError(reason);
  }
}

/**
 * Starts custom-scheme delivery after the React surface exists. Native URLs
 * never navigate the webview directly: Rust validates the public IDs and
 * current local library before this module emits an in-app navigation event.
 */
export async function startMobileDeepLinks(): Promise<() => void> {
  const unlisten = await onOpenUrl((urls) => {
    for (const url of urls) void openMobileDeepLink(url);
  });

  try {
    const launchUrls = await getCurrent();
    for (const url of launchUrls ?? []) await openMobileDeepLink(url);
  } catch (reason) {
    unlisten();
    throw reason;
  }

  return unlisten;
}
