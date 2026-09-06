import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { useSyncExternalStore } from "react";
import { isDesktop } from "./api";

export type AppUpdatePhase =
  | "disabled"
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "downloading"
  | "installing"
  | "error";

export type AppUpdateSnapshot = {
  enabled: boolean;
  phase: AppUpdatePhase;
  currentVersion: string | null;
  nextVersion: string | null;
  progress: number | null;
  message: string | null;
};

const previewUpdate =
  import.meta.env.DEV && import.meta.env.VITE_NOTED_UPDATE_PREVIEW === "available";
const updatesEnabled =
  previewUpdate || (isDesktop && import.meta.env.PROD && import.meta.env.VITE_NOTED_UPDATES === "1");

let snapshot: AppUpdateSnapshot = {
  enabled: updatesEnabled,
  phase: previewUpdate ? "available" : updatesEnabled ? "idle" : "disabled",
  currentVersion: previewUpdate ? "0.1.0" : null,
  nextVersion: previewUpdate ? "0.2.0" : null,
  progress: null,
  message: null,
};
let availableUpdate: Update | null = null;
let checking: Promise<void> | null = null;
let started = false;
let lastCheckedAt = 0;
let downloadedBytes = 0;
let downloadContentLength = 0;
const listeners = new Set<() => void>();
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

function publish(patch: Partial<AppUpdateSnapshot>) {
  snapshot = { ...snapshot, ...patch };
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function downloadProgress(event: DownloadEvent) {
  if (event.event === "Started") {
    publish({
      phase: "downloading",
      progress: event.data.contentLength ? 0 : null,
      message: "Downloading update",
    });
    return;
  }
  if (event.event === "Progress") {
    const current = snapshot.progress ?? 0;
    const total = downloadContentLength;
    downloadedBytes += event.data.chunkLength;
    publish({
      progress: total
        ? Math.min(100, Math.round((downloadedBytes / total) * 100))
        : current,
    });
    return;
  }
  publish({ phase: "installing", progress: 100, message: "Installing update" });
}

async function readCurrentVersion() {
  if (!updatesEnabled || snapshot.currentVersion) return;
  try {
    publish({ currentVersion: await getVersion() });
  } catch {
    // A missing version should not prevent an update check.
  }
}

export function checkForAppUpdate(options: { quiet?: boolean } = {}) {
  if (!updatesEnabled) return Promise.resolve();
  if (previewUpdate) {
    publish({ phase: "available", currentVersion: "0.1.0", nextVersion: "0.2.0" });
    return Promise.resolve();
  }
  if (checking) return checking;
  if (snapshot.phase === "downloading" || snapshot.phase === "installing") {
    return Promise.resolve();
  }

  const previousPhase = snapshot.phase;
  checking = (async () => {
    await readCurrentVersion();
    if (!options.quiet || previousPhase === "idle") {
      publish({ phase: "checking", message: null, progress: null });
    }
    try {
      const next = await check({ timeout: 15_000 });
      lastCheckedAt = Date.now();
      if (availableUpdate && availableUpdate !== next) void availableUpdate.close();
      availableUpdate = next;
      if (next) {
        publish({
          phase: "available",
          currentVersion: next.currentVersion,
          nextVersion: next.version,
          progress: null,
          message: next.body?.trim() || null,
        });
      } else {
        publish({ phase: "current", nextVersion: null, progress: null, message: null });
      }
    } catch (reason) {
      publish({
        phase: "error",
        progress: null,
        message: reason instanceof Error ? reason.message : String(reason),
      });
    } finally {
      checking = null;
    }
  })();
  return checking;
}

export async function installAppUpdate() {
  if (!updatesEnabled || snapshot.phase === "downloading" || snapshot.phase === "installing") {
    return;
  }
  if (!availableUpdate) await checkForAppUpdate();
  if (previewUpdate) {
    publish({ phase: "downloading", progress: 42, message: "Downloading update" });
    return;
  }
  if (!availableUpdate) return;

  downloadedBytes = 0;
  downloadContentLength = 0;
  try {
    await availableUpdate.downloadAndInstall((event) => {
      if (event.event === "Started") {
        downloadContentLength = event.data.contentLength ?? 0;
      }
      downloadProgress(event);
    });
    publish({ phase: "installing", progress: 100, message: "Restarting Noted" });
    await relaunch();
  } catch (reason) {
    publish({
      phase: "error",
      progress: null,
      message: reason instanceof Error ? reason.message : String(reason),
    });
  }
}

export function startAppUpdateChecks() {
  if (!updatesEnabled || started) return;
  started = true;
  if (previewUpdate) return;
  void checkForAppUpdate({ quiet: true });

  window.setInterval(() => void checkForAppUpdate({ quiet: true }), CHECK_INTERVAL_MS);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && Date.now() - lastCheckedAt >= CHECK_INTERVAL_MS) {
      void checkForAppUpdate({ quiet: true });
    }
  });
}

export function useAppUpdate() {
  return useSyncExternalStore(subscribe, () => snapshot, () => snapshot);
}
