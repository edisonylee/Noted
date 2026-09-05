import { useEffect, useState, type FormEvent } from "react";
import { Copy, Plus, Users } from "lucide-react";
import { api, isDesktop } from "../api";
import { team, orgPath, copyTeamText } from "./client";
import { TeamIntegrations } from "./TeamIntegrations";
import { TeamDialog } from "./TeamDialog";
import { collectionName } from "./presentation";
import type {
  TeamSnapshot,
  TeamGroup,
  TeamGrant,
  TeamRecipe,
  TeamSpace,
} from "./types";

type Invitation = {
  id: string;
  name: string;
  role: string;
  expires_at: number;
};
type Activity = {
  id: number;
  action: string;
  target_id: string;
  at: string;
  actor: string;
};
export function TeamAdministration({
  data,
  refresh,
}: {
  data: TeamSnapshot;
  refresh: () => Promise<TeamSnapshot>;
}) {
  const [tab, setTab] = useState("members"),
    [error, setError] = useState("");
  const [busy, setBusy] = useState(false),
    [message, setMessage] = useState("");
  const [invitations, setInvitations] = useState<Invitation[]>([]),
    [inviteOpen, setInviteOpen] = useState(false);
  const [inviteName, setInviteName] = useState(""),
    [inviteRole, setInviteRole] = useState("member"),
    [inviteCode, setInviteCode] = useState("");
  const [groups, setGroups] = useState<TeamGroup[]>([]),
    [groupEdit, setGroupEdit] = useState<TeamGroup | null>(null);
  const [promptEdit, setPromptEdit] = useState<TeamRecipe | null>(null);
  const [spaceEdit, setSpaceEdit] = useState<TeamSpace | null>(null);
  const [activity, setActivity] = useState<Activity[]>([]);
  const [teamName, setTeamName] = useState(data.org.name);
  const org = data.org.id,
    admin = data.org.role !== "member";
  useEffect(() => {
    if (!admin) {
      setInvitations([]);
      setGroups([]);
      setActivity([]);
      setGroupEdit(null);
      setSpaceEdit(null);
      setInviteOpen(false);
      setInviteCode("");
    }
  }, [admin]);
  const loadExtra = async () => {
    if (!admin) return;
    if (tab === "members")
      setInvitations(await team.request("GET", orgPath(org, "/invites")));
    if (tab === "groups" || tab === "spaces")
      setGroups(await team.request("GET", orgPath(org, "/groups")));
    if (tab === "activity")
      setActivity(await team.request("GET", orgPath(org, "/activity")));
  };
  useEffect(() => {
    let active = true;
    if (admin) {
      const path =
        tab === "members"
          ? "/invites"
          : tab === "groups" || tab === "spaces"
            ? "/groups"
            : tab === "activity"
              ? "/activity"
              : "";
      if (path)
        team
          .request<Invitation[] | TeamGroup[] | Activity[]>(
            "GET",
            orgPath(org, path),
          )
          .then((v) => {
            if (active) {
              if (path === "/invites") setInvitations(v as Invitation[]);
              else if (path === "/groups") setGroups(v as TeamGroup[]);
              else setActivity(v as Activity[]);
            }
          })
          .catch((e) => {
            if (active) setError(String(e));
          });
    }
    return () => {
      active = false;
    };
  }, [org, tab, admin]);
  const act = async (operation: () => Promise<unknown>, success = "") => {
    if (busy) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await operation();
      await refresh();
      await loadExtra();
      setMessage(success);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const invite = async (e: FormEvent) => {
    e.preventDefault();
    await act(async () => {
      const result = await team.request<{ token: string }>(
        "POST",
        orgPath(org, "/invites"),
        { name: inviteName, role: inviteRole },
      );
      setInviteCode(result.token);
      setInviteName("");
    });
  };
  return (
    <section className="team-admin">
      <header className="team-library-head">
        <div>
          <h1>Team settings</h1>
          <p>
            {data.org.name} · Signed in as {data.user.name}
          </p>
        </div>
      </header>
      {admin && (
        <form
          className="team-name-form"
          onSubmit={(event) => {
            event.preventDefault();
            void act(
              () => team.request("PATCH", orgPath(org), { name: teamName }),
              "Team name updated.",
            );
          }}
        >
          <label>
            Team name
            <input
              value={teamName}
              maxLength={200}
              required
              onChange={(event) => setTeamName(event.target.value)}
            />
          </label>
          <button
            className="team-primary"
            disabled={
              busy || !teamName.trim() || teamName.trim() === data.org.name
            }
          >
            Save name
          </button>
        </form>
      )}
      <nav className="team-tabs" aria-label="Team settings">
        {[
          "members",
          ...(admin ? ["spaces", "groups"] : []),
          "prompts",
          ...(admin ? ["integrations", "activity"] : []),
        ].map((name) => (
          <button
            key={name}
            className={tab === name ? "on" : ""}
            onClick={() => {
              setTab(name);
              setError("");
              setMessage("");
            }}
          >
            {name === "prompts"
              ? "Recipes & templates"
              : name === "spaces"
                ? "Collections"
                : name.charAt(0).toUpperCase() + name.slice(1)}
          </button>
        ))}
      </nav>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
      {message && (
        <p role="status" className="team-muted">
          {message}
        </p>
      )}
      {tab === "members" && (
        <>
          <div className="team-section-head">
            <h2>{data.members.length} members</h2>
            {admin && (
              <button
                className="team-text-button"
                onClick={() => {
                  setInviteCode("");
                  setInviteOpen(true);
                }}
              >
                <Plus size={14} /> Invite teammate
              </button>
            )}
          </div>
          <div className="team-member-list">
            {data.members.map((member) => (
              <div key={member.id} className="team-member-row">
                <Users size={16} />
                <div>
                  <strong>
                    {member.name}
                    {member.id === data.user.id ? " (you)" : ""}
                  </strong>
                  <small>
                    {member.role === "owner"
                      ? "Team owner"
                      : member.role === "admin"
                        ? "Admin"
                        : "Member"}
                  </small>
                </div>
                {admin &&
                  member.role !== "owner" &&
                  (data.org.role === "owner" ||
                    member.role !== "admin" ||
                    member.id === data.user.id) && (
                    <select
                      aria-label={`Role for ${member.name}`}
                      value={member.role}
                      disabled={busy}
                      onChange={(e) => {
                        const role = e.target.value;
                        if (
                          role === "remove" &&
                          !confirm(
                            `Remove ${member.name} from ${data.org.name}? They will lose access to all shared content in this team.`,
                          )
                        )
                          return;
                        void act(() =>
                          team.request(
                            "PATCH",
                            orgPath(org, `/members/${member.id}`),
                            { role },
                          ),
                        );
                      }}
                    >
                      <option value="member">Member</option>
                      <option value="admin">Admin</option>
                      <option value="remove">Remove access</option>
                    </select>
                  )}
                {data.org.role === "owner" && member.id !== data.user.id && (
                  <button
                    className="team-text-button"
                    disabled={busy}
                    onClick={() => {
                      if (
                        confirm(
                          `Make ${member.name} the team owner? You will remain an admin.`,
                        )
                      )
                        void act(() =>
                          team.request("POST", orgPath(org, "/owner"), {
                            user_id: member.id,
                          }),
                        );
                    }}
                  >
                    Make owner
                  </button>
                )}
              </div>
            ))}
          </div>
          {invitations.length > 0 && admin && (
            <>
              <h2 className="team-section-title">Pending invitations</h2>
              {invitations.map((invite) => (
                <div className="team-member-row" key={invite.id}>
                  <div>
                    <strong>{invite.name}</strong>
                    <small>
                      {invite.role} · Expires{" "}
                      {new Date(invite.expires_at).toLocaleDateString()}
                    </small>
                  </div>
                  <button
                    className="team-text-button"
                    disabled={busy}
                    onClick={() =>
                      void act(() =>
                        team.request(
                          "DELETE",
                          orgPath(org, `/invites/${invite.id}`),
                        ),
                      )
                    }
                  >
                    Revoke
                  </button>
                </div>
              ))}
            </>
          )}
          <p className="team-muted">
            Joining gives access to collections shared with all members.
            Restricted collections need a separate grant. Admins can access and
            manage all published content.
          </p>
          <div className="team-account-actions">
            <button
              className="team-text-button"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  const r = await team.request<{ token: string }>(
                    "POST",
                    orgPath(org, "/access-keys"),
                  );
                  setInviteCode(r.token);
                  setInviteOpen(true);
                  setInviteName("access-key");
                })
              }
            >
              Create access key for another Mac
            </button>
            {data.org.role !== "owner" && (
              <button
                className="team-text-button"
                disabled={busy}
                onClick={() => {
                  if (
                    confirm(
                      `Leave ${data.org.name}? You will need a new invitation to regain access.`,
                    )
                  )
                    void act(() =>
                      team.request(
                        "PATCH",
                        orgPath(org, `/members/${data.user.id}`),
                        { role: "remove" },
                      ),
                    );
                }}
              >
                Leave team
              </button>
            )}
          </div>
        </>
      )}
      {tab === "spaces" && admin && (
        <>
          <p className="team-muted">
            Collection permissions apply to every folder, meeting, search
            result, and answer within it.
          </p>
          {data.spaces.map((space) => (
            <div className="team-member-row" key={space.id}>
              <div>
                <strong>{collectionName(space)}</strong>
                <small>
                  {space.visibility === "team"
                    ? "Everyone in this team"
                    : "Admins and invited members or groups"}
                </small>
              </div>
              <button
                className="team-text-button"
                onClick={() => setSpaceEdit(space)}
              >
                Manage access
              </button>
            </div>
          ))}
        </>
      )}
      {tab === "groups" && admin && (
        <>
          <div className="team-section-head">
            <h2>User groups</h2>
            <button
              className="team-text-button"
              onClick={() => setGroupEdit({ id: "", name: "", member_ids: [] })}
            >
              <Plus size={14} /> Create group
            </button>
          </div>
          <p className="team-muted">
            Give a department or project team access together. Changes to group
            membership apply immediately.
          </p>
          {groups.map((group) => (
            <div className="team-member-row" key={group.id}>
              <div>
                <strong>{group.name}</strong>
                <small>
                  {group.member_ids.length}{" "}
                  {group.member_ids.length === 1 ? "member" : "members"}
                </small>
              </div>
              <button
                className="team-text-button"
                onClick={() => setGroupEdit(group)}
              >
                Edit
              </button>
              <button
                className="team-text-button"
                disabled={busy}
                onClick={() => {
                  if (
                    confirm(
                      `Delete ${group.name}? Access granted through this group will be removed.`,
                    )
                  )
                    void act(() =>
                      team.request(
                        "DELETE",
                        orgPath(org, `/groups/${group.id}`),
                      ),
                    );
                }}
              >
                Delete
              </button>
            </div>
          ))}
          {!groups.length && <p className="team-empty">No groups yet.</p>}
        </>
      )}
      {tab === "prompts" && (
        <>
          <div className="team-section-head">
            <h2>Shared prompts</h2>
            <button
              className="team-text-button"
              onClick={() =>
                setPromptEdit({
                  id: "",
                  name: "",
                  prompt: "",
                  kind: "recipe",
                  owner_id: data.user.id,
                  revision: 0,
                })
              }
            >
              <Plus size={14} /> New prompt
            </button>
          </div>
          <p className="team-muted">
            Recipes ask repeatable questions across shared meetings. Templates
            can be added to your local meeting summary formats.
          </p>
          {data.recipes.map((recipe) => (
            <div className="team-prompt-row" key={recipe.id}>
              <div>
                <strong>{recipe.name}</strong>
                <small>
                  {recipe.kind === "recipe" ? "Recipe" : "Summary template"}
                </small>
                <p>{recipe.prompt}</p>
              </div>
              <div className="team-prompt-actions">
                {recipe.kind === "template" && isDesktop && (
                  <button
                    className="team-text-button"
                    onClick={() =>
                      void act(async () => {
                        const templates = await api.meetingTemplates();
                        const name = `${data.org.name}: ${recipe.name}`;
                        if (
                          templates.some((t) => t.name === name) &&
                          !confirm(`Replace your local template “${name}”?`)
                        )
                          return;
                        await api.meetingTemplateSave(name, recipe.prompt);
                      }, "Template added to your meeting formats")
                    }
                  >
                    Use locally
                  </button>
                )}
                {(admin || recipe.owner_id === data.user.id) && (
                  <>
                    <button
                      className="team-text-button"
                      onClick={() => setPromptEdit(recipe)}
                    >
                      Edit
                    </button>
                    <button
                      className="team-text-button"
                      disabled={busy}
                      onClick={() => {
                        if (confirm(`Delete shared prompt “${recipe.name}”?`))
                          void act(() =>
                            team.request(
                              "DELETE",
                              orgPath(org, `/recipes/${recipe.id}`),
                            ),
                          );
                      }}
                    >
                      Delete
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
          {!data.recipes.length && (
            <p className="team-empty">
              Save a question your team asks often, such as “What decisions
              changed this week?”
            </p>
          )}
        </>
      )}
      {tab === "integrations" && admin && <TeamIntegrations data={data} />}
      {tab === "activity" && admin && (
        <>
          <h2 className="team-section-title">Recent team activity</h2>
          <div className="team-activity">
            {activity.map((a) => (
              <div key={a.id}>
                <strong>{a.actor}</strong>
                <span>{a.action.replace(/[._]/g, " ")}</span>
                <time>{new Date(a.at).toLocaleString()}</time>
              </div>
            ))}
          </div>
        </>
      )}
      {inviteOpen && (
        <TeamDialog
          title={
            inviteName === "access-key"
              ? "Access key for another Mac"
              : "Invite a teammate"
          }
          onClose={() => {
            setInviteOpen(false);
            setInviteCode("");
            setInviteName("");
          }}
        >
          {inviteCode ? (
            <div className="team-form">
              <p>
                {inviteName === "access-key"
                  ? "Use this key to sign in on your other Mac. It expires in 30 days and grants your account’s access."
                  : "Send this code to your teammate along with your team server address. It works once and expires in seven days."}
              </p>
              <label>
                {inviteName === "access-key" ? "Access key" : "Invitation code"}
                <input
                  readOnly
                  value={inviteCode}
                  onFocus={(e) => e.target.select()}
                />
              </label>
              <button
                className="team-primary"
                onClick={() =>
                  void copyTeamText(inviteCode)
                    .then(() => setMessage("Code copied"))
                    .catch((e) => setError(String(e)))
                }
              >
                <Copy size={14} /> Copy code
              </button>
              {message && <p role="status">{message}</p>}
              {error && (
                <p className="team-error" role="alert">
                  {error}
                </p>
              )}
            </div>
          ) : (
            <form className="team-form" onSubmit={invite}>
              <label>
                Teammate’s name
                <input
                  autoFocus
                  required
                  maxLength={200}
                  value={inviteName}
                  onChange={(e) => setInviteName(e.target.value)}
                />
              </label>
              <label>
                Role
                <select
                  value={inviteRole}
                  onChange={(e) => setInviteRole(e.target.value)}
                >
                  <option value="member">Member</option>
                  <option value="admin">Admin</option>
                </select>
              </label>
              <p className="team-muted">
                The code grants the chosen role to whoever receives it.
              </p>
              {error && (
                <p className="team-error" role="alert">
                  {error}
                </p>
              )}
              <button className="team-primary" disabled={busy}>
                Create invitation
              </button>
            </form>
          )}
        </TeamDialog>
      )}
      {groupEdit && (
        <GroupEditor
          group={groupEdit}
          data={data}
          onClose={() => setGroupEdit(null)}
          onSaved={async () => {
            setGroupEdit(null);
            await loadExtra();
          }}
        />
      )}
      {promptEdit && (
        <PromptEditor
          prompt={promptEdit}
          org={org}
          onClose={() => setPromptEdit(null)}
          onSaved={async () => {
            setPromptEdit(null);
            await refresh();
          }}
        />
      )}
      {spaceEdit && (
        <SpaceAccess
          space={spaceEdit}
          data={data}
          groups={groups}
          onClose={() => setSpaceEdit(null)}
          refresh={refresh}
        />
      )}
    </section>
  );
}
function GroupEditor({
  group,
  data,
  onClose,
  onSaved,
}: {
  group: TeamGroup;
  data: TeamSnapshot;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [name, setName] = useState(group.name),
    [members, setMembers] = useState(group.member_ids),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  return (
    <TeamDialog
      title={group.id ? "Edit group" : "Create group"}
      onClose={onClose}
    >
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          try {
            await team.request(
              group.id ? "PUT" : "POST",
              orgPath(data.org.id, `/groups${group.id ? `/${group.id}` : ""}`),
              { name, member_ids: members },
            );
            await onSaved();
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Group name
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            maxLength={200}
            autoFocus
          />
        </label>
        <fieldset>
          <legend>Members</legend>
          {data.members.map((m) => (
            <label key={m.id} className="team-checkbox">
              <input
                type="checkbox"
                checked={members.includes(m.id)}
                onChange={(e) =>
                  setMembers((old) =>
                    e.target.checked
                      ? [...old, m.id]
                      : old.filter((id) => id !== m.id),
                  )
                }
              />
              {m.name}
            </label>
          ))}
        </fieldset>
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy}>
          Save group
        </button>
      </form>
    </TeamDialog>
  );
}
function PromptEditor({
  prompt,
  org,
  onClose,
  onSaved,
}: {
  prompt: TeamRecipe;
  org: string;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [value, setValue] = useState(prompt),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  return (
    <TeamDialog
      title={prompt.id ? "Edit shared prompt" : "New shared prompt"}
      onClose={onClose}
    >
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          try {
            await team.request(
              prompt.id ? "PUT" : "POST",
              orgPath(org, `/recipes${prompt.id ? `/${prompt.id}` : ""}`),
              value,
            );
            await onSaved();
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Name
          <input
            required
            maxLength={200}
            autoFocus
            value={value.name}
            onChange={(e) => setValue({ ...value, name: e.target.value })}
          />
        </label>
        <label>
          Type
          <select
            value={value.kind}
            onChange={(e) =>
              setValue({ ...value, kind: e.target.value as TeamRecipe["kind"] })
            }
          >
            <option value="recipe">Recipe for shared meeting chat</option>
            <option value="template">Meeting summary template</option>
          </select>
        </label>
        <label>
          Instructions
          <textarea
            required
            maxLength={12000}
            rows={8}
            value={value.prompt}
            onChange={(e) => setValue({ ...value, prompt: e.target.value })}
          />
        </label>
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy}>
          Save shared prompt
        </button>
      </form>
    </TeamDialog>
  );
}
function SpaceAccess({
  space,
  data,
  groups,
  onClose,
  refresh,
}: {
  space: TeamSpace;
  data: TeamSnapshot;
  groups: TeamGroup[];
  onClose: () => void;
  refresh: () => Promise<TeamSnapshot>;
}) {
  const [value, setValue] = useState({ ...space, name: collectionName(space) }),
    [grants, setGrants] = useState<TeamGrant[]>([]),
    [recipient, setRecipient] = useState(""),
    [role, setRole] = useState("viewer");
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const path = orgPath(data.org.id, `/spaces/${space.id}`);
  useEffect(() => {
    team
      .request<TeamGrant[]>("GET", `${path}/grants`)
      .then(setGrants)
      .catch((e) => setError(String(e)));
  }, [path]);
  const grant = async (kind: string, id: string, role: string) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      setGrants(
        await team.request("PUT", `${path}/grants`, { kind, id, role }),
      );
      await refresh();
      setRecipient("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <TeamDialog title={`Access to ${collectionName(space)}`} onClose={onClose}>
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          try {
            await team.request("PATCH", path, value);
            await refresh();
            onClose();
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Collection name
          <input
            required
            maxLength={200}
            value={value.name}
            onChange={(e) => setValue({ ...value, name: e.target.value })}
          />
        </label>
        <label>
          Description
          <textarea
            rows={2}
            maxLength={4000}
            value={value.description}
            onChange={(e) =>
              setValue({ ...value, description: e.target.value })
            }
          />
        </label>
        <label>
          Visibility
          <select
            value={value.visibility}
            onChange={(e) =>
              setValue({
                ...value,
                visibility: e.target.value as TeamSpace["visibility"],
              })
            }
          >
            <option value="restricted">
              Admins and invited members or groups
            </option>
            <option value="team">Everyone in this team</option>
          </select>
        </label>
        <label className="team-checkbox">
          <input
            type="checkbox"
            checked={value.api_enabled}
            onChange={(e) =>
              setValue({ ...value, api_enabled: e.target.checked })
            }
          />
          Allow approved team integrations to read this collection
        </label>
        <p className="team-muted">
          Each integration still needs a key explicitly scoped to this
          collection. Turning this off immediately blocks all integration keys
          here.
        </p>
        <button className="team-primary" disabled={busy}>
          Save collection settings
        </button>
      </form>
      <h3>Members and groups</h3>
      {space.visibility === "team" && (
        <p className="team-muted">
          Everyone in this team is an editor unless given an explicit viewer
          grant. Removing a grant restores that default access.
        </p>
      )}
      <p className="team-muted">
        Admins always have access. Viewers can read and search. Editors can
        publish and edit shared notes.
      </p>
      {grants.map((g) => (
        <div key={`${g.kind}:${g.id}`} className="team-member-row">
          <div>
            <strong>{g.name}</strong>
            <small>{g.kind}</small>
          </div>
          <select
            aria-label={`Access for ${g.name}`}
            value={g.role}
            disabled={busy}
            onChange={(e) => void grant(g.kind, g.id, e.target.value)}
          >
            <option value="viewer">Viewer</option>
            <option value="editor">Editor</option>
            <option value="remove">Remove grant</option>
          </select>
        </div>
      ))}
      <form
        className="team-form"
        onSubmit={(e) => {
          e.preventDefault();
          const [kind, id] = recipient.split(":");
          if (id) void grant(kind, id, role);
        }}
      >
        <label>
          Add access
          <select
            value={recipient}
            required
            onChange={(e) => setRecipient(e.target.value)}
          >
            <option value="">Choose a member or group</option>
            <optgroup label="Members">
              {data.members
                .filter((m) => m.role === "member")
                .map((m) => (
                  <option key={m.id} value={`member:${m.id}`}>
                    {m.name}
                  </option>
                ))}
            </optgroup>
            <optgroup label="Groups">
              {groups.map((g) => (
                <option key={g.id} value={`group:${g.id}`}>
                  {g.name}
                </option>
              ))}
            </optgroup>
          </select>
        </label>
        <label>
          Access
          <select value={role} onChange={(e) => setRole(e.target.value)}>
            <option value="viewer">Viewer</option>
            <option value="editor">Editor</option>
          </select>
        </label>
        <button className="team-text-button" disabled={busy || !recipient}>
          Grant access
        </button>
      </form>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
    </TeamDialog>
  );
}
