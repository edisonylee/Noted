import { useEffect, useState } from "react";
import { team } from "./teams/client";
import { TeamProfileSettings } from "./teams/TeamProfile";
import "./teams/teams.css";

export function ProfileSettings({ onOpenTeam }: { onOpenTeam?: () => void }) {
  const [connected, setConnected] = useState<boolean | null>(null);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    let active = true;
    team.status().then((status) => {
      if (active) {
        setConnected(status.connected);
        setError("");
      }
    }).catch((error) => {
      if (active) setError(String(error));
    });
    return () => { active = false; };
  }, [retry]);

  if (connected) return <TeamProfileSettings />;

  return (
    <section>
      <h3>Profile</h3>
      {error ? (
        <p className="team-error" role="alert">
          {error}{" "}
          <button className="team-text-button" onClick={() => setRetry((value) => value + 1)}>
            Try again
          </button>
        </p>
      ) : connected === null ? (
        <p className="settings-sub" role="status">Loading your profile…</p>
      ) : (
        <>
          <p className="settings-sub">
            Your profile holds the photo, name, and details you share with teammates.
            Connect to your team to create or edit it.
          </p>
          {onOpenTeam ? (
            <button className="ghost-btn" onClick={onOpenTeam}>Connect to a team</button>
          ) : (
            <p className="field-hint">Open Team in Noted on your Mac to connect.</p>
          )}
        </>
      )}
    </section>
  );
}
