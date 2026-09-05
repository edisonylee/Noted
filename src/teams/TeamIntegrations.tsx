import { useCallback, useEffect, useState } from "react";
import { Copy, Plus } from "lucide-react";
import { team, orgPath, copyTeamText } from "./client";
import { TeamDialog } from "./TeamDialog";
import { collectionName } from "./presentation";
import type { TeamSnapshot } from "./types";

type Key = {
  id: string;
  name: string;
  transcripts: boolean;
  created_at: string;
  expires_at: number;
  revoked_at: string | null;
  space_ids: string[];
};
export function TeamIntegrations({ data }: { data: TeamSnapshot }) {
  const [keys, setKeys] = useState<Key[]>([]),
    [open, setOpen] = useState(false);
  const [name, setName] = useState(""),
    [spaces, setSpaces] = useState<string[]>([]),
    [days, setDays] = useState(30),
    [transcripts, setTranscripts] = useState(false);
  const [token, setToken] = useState(""),
    [server, setServer] = useState("");
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [message, setMessage] = useState("");
  const path = orgPath(data.org.id, "/integrations");
  const reload = useCallback(
    async () => setKeys(await team.request<Key[]>("GET", path)),
    [path],
  );
  useEffect(() => {
    let active = true;
    Promise.all([team.request<Key[]>("GET", path), team.status()])
      .then(([values, status]) => {
        if (active) {
          setKeys(values);
          setServer(status.server);
        }
      })
      .catch((e) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, [path]);
  return (
    <section>
      <div className="team-section-head">
        <h2>Team integrations</h2>
        <button
          className="team-text-button"
          onClick={() => {
            setName("");
            setSpaces([]);
            setToken("");
            setError("");
            setMessage("");
            setTranscripts(false);
            setOpen(true);
          }}
        >
          <Plus size={14} /> Create read-only key
        </button>
      </div>
      <p className="team-muted">
        Give each tool its own key and selected collections. These keys belong
        to the team and keep working when their creator leaves. They cannot edit
        notes, manage members, or read private saved answers.
      </p>
      <p className="team-muted">
        Enable integration access in a collection’s settings first. It starts
        off for every collection. Transcript access is a separate choice for
        each key.
      </p>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
      {keys.map((key) => (
        <div className="team-member-row" key={key.id}>
          <div>
            <strong>{key.name}</strong>
            <small>
              {key.space_ids
                .map(
                  (id) =>
                    (() => {
                      const collection = data.spaces.find((s) => s.id === id);
                      return collection
                        ? collectionName(collection)
                        : undefined;
                    })() ?? "Unavailable collection",
                )
                .join(", ")}{" "}
              ·{" "}
              {key.transcripts ? "Summaries and transcripts" : "Summaries only"}
            </small>
            <small>
              {key.revoked_at
                ? "Revoked"
                : Date.now() >= key.expires_at
                  ? "Expired"
                  : `Expires ${new Date(key.expires_at).toLocaleDateString()}`}
            </small>
          </div>
          {!key.revoked_at && (
            <button
              className="team-text-button"
              disabled={busy}
              onClick={async () => {
                if (
                  !confirm(
                    `Revoke ${key.name}? This tool will immediately lose team access.`,
                  )
                )
                  return;
                setBusy(true);
                try {
                  await team.request("DELETE", `${path}/${key.id}`);
                  await reload();
                } catch (e) {
                  setError(String(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              Revoke key
            </button>
          )}
        </div>
      ))}
      {!keys.length && (
        <p className="team-empty">No integration has access yet.</p>
      )}
      <details className="team-transcript">
        <summary>API endpoints</summary>
        <p className="team-muted">
          Send the key in the Authorization header as a Bearer token. List
          endpoints accept q, space, folder and offset; only approved
          collections are visible.
        </p>
        <pre>{`GET ${server}/v1/api/spaces\nGET ${server}/v1/api/folders\nGET ${server}/v1/api/notes?q=launch\nGET ${server}/v1/api/notes/{meeting-id}`}</pre>
      </details>
      {open && (
        <TeamDialog
          title={
            token ? "Your integration key" : "Create a read-only integration"
          }
          busy={busy}
          onClose={() => {
            setOpen(false);
            setToken("");
          }}
        >
          {token ? (
            <div className="team-form">
              <p>
                Save this key in the tool’s secret storage. It is shown only
                once.
              </p>
              <label>
                Integration key
                <input
                  readOnly
                  value={token}
                  onFocus={(e) => e.target.select()}
                />
              </label>
              <button
                className="team-primary"
                onClick={() =>
                  void copyTeamText(token)
                    .then(() => setMessage("Key copied"))
                    .catch((e) => setError(String(e)))
                }
              >
                <Copy size={14} /> Copy key
              </button>
              {message && <p role="status">{message}</p>}
              {error && (
                <p className="team-error" role="alert">
                  {error}
                </p>
              )}
            </div>
          ) : (
            <form
              className="team-form"
              onSubmit={async (e) => {
                e.preventDefault();
                if (busy) return;
                setBusy(true);
                setError("");
                try {
                  const created = await team.request<{ token: string }>(
                    "POST",
                    path,
                    { name, space_ids: spaces, days, transcripts },
                  );
                  setToken(created.token);
                  await reload();
                } catch (e) {
                  setError(String(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              <label>
                Tool name
                <input
                  required
                  maxLength={200}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="Internal research assistant"
                />
              </label>
              <fieldset>
                <legend>Approved collections</legend>
                {data.spaces
                  .filter((s) => s.api_enabled)
                  .map((space) => (
                    <label key={space.id} className="team-checkbox">
                      <input
                        type="checkbox"
                        checked={spaces.includes(space.id)}
                        onChange={(e) =>
                          setSpaces((old) =>
                            e.target.checked
                              ? [...old, space.id]
                              : old.filter((id) => id !== space.id),
                          )
                        }
                      />
                      {collectionName(space)}
                    </label>
                  ))}
                {!data.spaces.some((s) => s.api_enabled) && (
                  <p className="team-muted">
                    Open Collections → Manage access and allow approved
                    integrations for a collection first.
                  </p>
                )}
              </fieldset>
              <label>
                Expires after
                <select
                  value={days}
                  onChange={(e) => setDays(Number(e.target.value))}
                >
                  <option value={7}>7 days</option>
                  <option value={30}>30 days</option>
                  <option value={90}>90 days</option>
                  <option value={365}>1 year</option>
                </select>
              </label>
              <label className="team-checkbox">
                <input
                  type="checkbox"
                  checked={transcripts}
                  onChange={(e) => setTranscripts(e.target.checked)}
                />
                Allow this tool to read and search published transcripts
              </label>
              <p className="team-muted">
                This grants access to current and future published meetings in
                the selected collections.
              </p>
              {error && (
                <p className="team-error" role="alert">
                  {error}
                </p>
              )}
              <button
                className="team-primary"
                disabled={busy || !spaces.length}
              >
                Create integration key
              </button>
            </form>
          )}
        </TeamDialog>
      )}
    </section>
  );
}
