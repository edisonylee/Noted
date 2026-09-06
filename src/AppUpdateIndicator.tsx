import { Download, Loader2 } from "lucide-react";
import { useEffect } from "react";
import { installAppUpdate, startAppUpdateChecks, useAppUpdate } from "./appUpdater";

export function AppUpdateIndicator() {
  const update = useAppUpdate();

  useEffect(() => startAppUpdateChecks(), []);

  if (!update.enabled || !["available", "downloading", "installing"].includes(update.phase)) {
    return null;
  }

  const busy = update.phase === "downloading" || update.phase === "installing";
  const detail =
    update.phase === "available"
      ? `Version ${update.nextVersion} is ready`
      : update.phase === "downloading" && update.progress != null
        ? `Downloading ${update.progress}%`
        : update.phase === "downloading"
          ? "Downloading"
          : "Restarting Noted";

  return (
    <button
      type="button"
      className="sidebar-update"
      onClick={() => void installAppUpdate()}
      disabled={busy}
      aria-label={busy ? detail : `Update Noted to version ${update.nextVersion} and restart`}
    >
      {busy ? <Loader2 size={15} className="spin" /> : <Download size={15} />}
      <span>
        <strong>{update.phase === "available" ? "Update Noted" : "Updating Noted"}</strong>
        <small aria-live="polite">{detail}</small>
      </span>
    </button>
  );
}
