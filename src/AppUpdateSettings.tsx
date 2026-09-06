import { Download, Loader2, RefreshCw } from "lucide-react";
import { useEffect } from "react";
import {
  checkForAppUpdate,
  installAppUpdate,
  startAppUpdateChecks,
  useAppUpdate,
} from "./appUpdater";

export function AppUpdateSettings() {
  const update = useAppUpdate();

  useEffect(() => startAppUpdateChecks(), []);
  if (!update.enabled) return null;

  const busy = ["checking", "downloading", "installing"].includes(update.phase);
  const available = update.phase === "available";
  const busyLabel =
    update.phase === "downloading"
      ? "Downloading"
      : update.phase === "installing"
        ? "Restarting"
        : "Checking";
  const status = (() => {
    if (update.phase === "idle" || update.phase === "checking") return "Checking for a newer version";
    if (update.phase === "current") return "You have the latest published beta";
    if (update.phase === "available") return `Version ${update.nextVersion} is ready to install`;
    if (update.phase === "downloading") {
      return update.progress == null
        ? "Downloading the update"
        : `Downloading the update: ${update.progress}%`;
    }
    if (update.phase === "installing") return "Installing, then restarting Noted";
    return "Noted could not check for updates. Your current version is unchanged.";
  })();

  return (
    <section className="settings-group app-update-settings">
      <header className="settings-group-head">
        <h4>Application updates</h4>
        <p>Published beta builds are signed, verified, and installed without changing your notes.</p>
      </header>
      <div className="app-update-row">
        <span className="app-update-copy">
          <strong>Noted {update.currentVersion ?? "beta"}</strong>
          <small className={update.phase === "error" ? "error" : ""} aria-live="polite">
            {status}
          </small>
        </span>
        {available ? (
          <button type="button" className="primary" onClick={() => void installAppUpdate()}>
            <Download size={14} /> Update and restart
          </button>
        ) : (
          <button
            type="button"
            className="ghost-btn"
            onClick={() => void checkForAppUpdate()}
            disabled={busy}
          >
            {busy ? <Loader2 size={14} className="spin" /> : <RefreshCw size={14} />}
            {busy ? busyLabel : "Check now"}
          </button>
        )}
      </div>
      {update.message && update.phase === "available" && (
        <p className="app-update-notes">{update.message}</p>
      )}
    </section>
  );
}
