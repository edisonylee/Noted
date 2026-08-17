import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import "./MobileShell.css";

type MobileHealth = {
  platform: "ios";
  storage: "not_initialized";
  sync: "not_enrolled";
};

export function MobileShell() {
  const [health, setHealth] = useState<MobileHealth | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<MobileHealth>("mobile_health")
      .then(setHealth)
      .catch((reason: unknown) => {
        setError(reason instanceof Error ? reason.message : String(reason));
      });
  }, []);

  const ready = health?.platform === "ios";

  return (
    <main className="mobile-shell">
      <section className="mobile-shell__card" aria-live="polite">
        <p className="mobile-shell__eyebrow">Noted for iPhone</p>
        <h1>Your companion is taking shape.</h1>
        <p className="mobile-shell__intro">
          This native shell is deliberately isolated from the Mac recorder, models,
          assistant, and LAN server.
        </p>

        <dl className="mobile-shell__status">
          <div>
            <dt>Native runtime</dt>
            <dd data-ready={ready}>{error ? "Unavailable" : ready ? "Ready" : "Checking…"}</dd>
          </div>
          <div>
            <dt>Local library</dt>
            <dd>Next phase</dd>
          </div>
          <div>
            <dt>Encrypted sync</dt>
            <dd>Not enrolled</dd>
          </div>
        </dl>

        {error && <p className="mobile-shell__error">Startup check failed: {error}</p>}
        <p className="mobile-shell__footnote">
          No personal data has been copied to this device yet.
        </p>
      </section>
    </main>
  );
}
