import { TeamSearch } from "./TeamSearch";
import type { TeamNotificationTarget } from "./types";
import {
  useNavigationState,
  clearNavigationState,
} from "../useNavigationState";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  ArrowLeft,
  BookOpen,
  Check,
  ChevronRight,
  Copy,
  Folder,
  Loader,
  Lock,
  LogOut,
  MessageSquare,
  Share2,
  ChevronDown,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Trash2,
  Users,
} from "lucide-react";
import { MdBlock } from "../MeetingMarkdownView";
import { team, orgPath, copyTeamText } from "./client";
import type {
  TeamOrg,
  TeamSnapshot,
  TeamSpace,
  TeamFolder,
  TeamNote,
  TeamNoteRow,
  TeamChatRoom,
} from "./types";
import { TeamDialog } from "./TeamDialog";
import { TeamMessages } from "./TeamMessages";
import { TeamPeople } from "./TeamPeople";
import { TeamAvatars } from "./TeamAvatar";
import { collectionName, collectionAudience } from "./presentation";
import { SavedAnswers } from "./SavedAnswers";
import { TeamChat } from "./TeamChat";
import { TeamAdministration } from "./TeamAdministration";
import "./teams.css";
import "./meetings-layout.css";
import "./team-layout.css";

export function TeamConnect({
  onConnected,
}: {
  onConnected: (orgs: TeamOrg[]) => void;
}) {
  const [server, setServer] = useState("");
  const [mode, setMode] = useState("join");
  const [secret, setSecret] = useState("");
  const [name, setName] = useState("");
  const [organization, setOrganization] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    team
      .status()
      .then((s) => setServer(s.server))
      .catch(() => {});
  }, []);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const orgs = await team.connect(server, mode, secret, organization, name);
      setSecret("");
      onConnected(orgs);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="team-connect">
      <h1>Your team’s meetings and conversations.</h1>
      <p>
        Share meeting notes, find answers in your company’s knowledge, and
        message your teammates.
      </p>
      <form onSubmit={submit} className="team-form">
        <label>
          Team server
          <input
            type="url"
            value={server}
            onChange={(e) => setServer(e.target.value)}
            placeholder="https://notes.yourcompany.com"
            required
            autoComplete="url"
          />
        </label>
        <label>
          Connection
          <select
            value={mode}
            onChange={(e) => {
              setMode(e.target.value);
              setSecret("");
            }}
          >
            <option value="join">Join with an invitation</option>
            <option value="signin">Sign in with an access key</option>
            <option value="create">Set up a new team server</option>
          </select>
        </label>
        {mode === "create" && (
          <>
            <label>
              Team name
              <input
                value={organization}
                onChange={(e) => setOrganization(e.target.value)}
                maxLength={200}
                required
              />
            </label>
            <label>
              Your name
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                maxLength={200}
                required
                autoComplete="name"
              />
            </label>
          </>
        )}
        <label>
          {mode === "join"
            ? "Invitation code"
            : mode === "create"
              ? "Server setup key"
              : "Access key"}
          <input
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            required
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy}>
          {busy ? <Loader size={15} className="spin" /> : null}
          {mode === "create" ? "Create team" : "Connect to team"}
        </button>
      </form>
      <p className="team-privacy">
        <Lock size={14} /> Your local library stays private. You choose which
        meetings to publish.
      </p>
    </section>
  );
}
export function TeamWorkspace({
  onOpenLibrary,
  notificationTarget,
  onNotificationHandled,
}: {
  onOpenLibrary?: () => void;
  notificationTarget?: TeamNotificationTarget | null;
  onNotificationHandled?: () => void;
} = {}) {
  const [orgs, setOrgs] = useState<TeamOrg[] | null>(null);
  const [org, setOrg] = useNavigationState("team:org", "");
  const [connected, setConnected] = useState<boolean | null>(null);
  const [error, setError] = useState("");
  const [addWorkspace, setAddWorkspace] = useState(false);
  const [validatedTarget, setValidatedTarget] =
    useState<TeamNotificationTarget | null>(null);
  useEffect(() => {
    let active = true;
    team
      .status()
      .then(async (status) => {
        if (!active) return;
        setConnected(status.connected);
        if (status.connected) {
          const values = await team.request<TeamOrg[]>("GET", "/v1/orgs");
          if (active) {
            setOrgs(values);
            setOrg((previous) =>
              values.some((value) => value.id === previous)
                ? previous
                : (values[0]?.id ?? ""),
            );
          }
        }
      })
      .catch((e) => {
        if (active) {
          setError(String(e));
          setConnected(false);
        }
      });
    return () => {
      active = false;
    };
  }, []);
  useEffect(() => {
    if (!notificationTarget) {
      setValidatedTarget(null);
      return;
    }
    if (!orgs) return;
    let alive = true;
    void team
      .status()
      .then((status) => {
        if (!alive) return;
        if (
          status.server.replace(/\/$/, "") !==
            notificationTarget.server.replace(/\/$/, "") ||
          !orgs.some((o) => o.id === notificationTarget.org)
        ) {
          setError(
            "This notification belongs to a team or server that is no longer connected.",
          );
          onNotificationHandled?.();
        } else {
          setOrg(notificationTarget.org);
          setValidatedTarget(notificationTarget);
        }
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [notificationTarget, orgs, onNotificationHandled]);
  const updateTeam = useCallback(
    (next: TeamOrg) =>
      setOrgs(
        (old) =>
          old?.map((team) => (team.id === next.id ? next : team)) ?? null,
      ),
    [],
  );
  if (connected == null)
    return (
      <div className="team-loading">
        <Loader className="spin" size={18} /> Opening your team…
      </div>
    );
  if (!connected)
    return (
      <>
        <TeamConnect
          onConnected={(values) => {
            setOrgs(values);
            setOrg(values[0]?.id ?? "");
            setConnected(true);
            setError("");
          }}
        />
        {error && <p className="team-error">{error}</p>}
      </>
    );
  return (
    <div className="team-workspace">
      <div className="team-workspace-bar">
        <label className="team-identity">
          <span className="team-eyebrow">Team</span>
          <select
            aria-label="Choose team"
            value={org}
            onChange={(e) => setOrg(e.target.value)}
          >
            {(orgs ?? []).map((o) => (
              <option key={o.id} value={o.id}>
                {o.name}
              </option>
            ))}
          </select>
        </label>

        <details className="team-account-menu">
          <summary aria-label="Team options">
            <ChevronDown size={16} />
          </summary>
          <div>
            <button
              className="team-text-button"
              onClick={(event) => {
                event.currentTarget.closest("details")?.removeAttribute("open");
                setAddWorkspace(true);
              }}
            >
              <Plus size={14} /> Create or join a team
            </button>
            <button
              className="team-text-button"
              onClick={async () => {
                try {
                  await team.disconnect();
                  clearNavigationState("team:");
                  setConnected(false);
                  setOrgs(null);
                  setOrg("");
                } catch (e) {
                  setError(String(e));
                }
              }}
            >
              <LogOut size={14} /> Sign out
            </button>
          </div>
        </details>
      </div>
      {error && (
        <p className="team-error" role="alert">
          {error}
        </p>
      )}
      {org ? (
        <TeamAvatars key={org}>
          <TeamLibrary
            key={org}
            org={org}
            onOpenLibrary={onOpenLibrary}
            onTeamUpdate={updateTeam}
            notificationTarget={
              validatedTarget === notificationTarget &&
              validatedTarget?.org === org
                ? validatedTarget
                : null
            }
            onNotificationHandled={onNotificationHandled}
          />
        </TeamAvatars>
      ) : (
        <p className="team-empty">
          You don’t have access to a team. Ask an admin for a new invitation.
        </p>
      )}
      {addWorkspace && (
        <AddWorkspace
          onClose={() => setAddWorkspace(false)}
          onAdded={async (id) => {
            const values = await team.request<TeamOrg[]>("GET", "/v1/orgs");
            setOrgs(values);
            setOrg(id);
            setAddWorkspace(false);
          }}
        />
      )}
    </div>
  );
}

function AddWorkspace({
  onClose,
  onAdded,
}: {
  onClose: () => void;
  onAdded: (id: string) => Promise<void>;
}) {
  const [mode, setMode] = useState("join"),
    [value, setValue] = useState("");
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  return (
    <TeamDialog title="Create or join a team" onClose={onClose}>
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          setError("");
          try {
            const result = await team.request<{ id?: string; org?: string }>(
              "POST",
              mode === "join" ? "/v1/orgs/join" : "/v1/orgs",
              mode === "join" ? { invitation: value } : { name: value },
            );
            await onAdded(result.org ?? result.id!);
          } catch (e) {
            setError(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Team action
          <select
            value={mode}
            onChange={(e) => {
              setMode(e.target.value);
              setValue("");
            }}
          >
            <option value="join">Join an existing team</option>
            <option value="create">Create a team</option>
          </select>
        </label>
        <label>
          {mode === "join" ? "Invitation code" : "Team name"}
          <input
            autoFocus
            required
            maxLength={200}
            type={mode === "join" ? "password" : "text"}
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
        </label>
        <p className="team-muted">
          Each team has its own members, meeting collections, and shared
          content.
        </p>
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy}>
          {mode === "join" ? "Join team" : "Create team"}
        </button>
      </form>
    </TeamDialog>
  );
}

function TeamLibrary({
  org,
  onOpenLibrary,
  onTeamUpdate,
  notificationTarget,
  onNotificationHandled,
}: {
  notificationTarget?: TeamNotificationTarget | null;
  onNotificationHandled?: () => void;
  org: string;
  onOpenLibrary?: () => void;
  onTeamUpdate: (team: TeamOrg) => void;
}) {
  const [data, setData] = useState<TeamSnapshot | null>(null);
  const [space, setSpace] = useNavigationState(`team:${org}:space`, "");
  const [folder, setFolder] = useNavigationState(`team:${org}:folder`, "");
  const [view, setView] = useNavigationState<
    "notes" | "admin" | "trash" | "answers" | "messages" | "people" | "search"
  >(`team:${org}:view`, "notes");
  const [searchRoom, setSearchRoom] = useNavigationState(
    `team:${org}:search-room`,
    "",
  );
  const [searchMeeting, setSearchMeeting] = useState<string | null>(null);
  const [requestedMessage, setRequestedMessage] = useState<{
    id: string;
    nonce: number;
  } | null>(null);
  const [unread, setUnread] = useState(0);
  useEffect(() => {
    if (notificationTarget) setView("messages");
  }, [notificationTarget]);
  const [requestedRoom, setRequestedRoom] = useState<TeamChatRoom | null>(null);
  const [shareHelp, setShareHelp] = useState(false);
  const [query, setQuery] = useNavigationState(`team:${org}:query`, "");
  const [rows, setRows] = useState<TeamNoteRow[]>([]);
  const [more, setMore] = useState(false);
  const [selected, setSelected] = useNavigationState<string[]>(
    `team:${org}:selected`,
    [],
  );
  const [noteId, setNoteId] = useNavigationState<string | null>(
    `team:${org}:note`,
    null,
  );
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [conversationId, setConversationId] = useNavigationState<string | null>(
    `team:${org}:conversation`,
    null,
  );
  const [editor, setEditor] = useState<
    "space" | "folder" | "editFolder" | null
  >(null);
  const [accessEpoch, setAccessEpoch] = useState(0);
  const accessVersion = useRef<number | null>(null);
  const loadedAccessEpoch = useRef(0);
  const requestVersion = useRef(0);
  const refresh = useCallback(async () => {
    const next = await team.request<TeamSnapshot>("GET", orgPath(org));
    if (
      accessVersion.current != null &&
      next.access_version < accessVersion.current
    ) {
      throw new Error(
        "Team access changed. Refresh again for the current version.",
      );
    }
    if (
      accessVersion.current != null &&
      accessVersion.current !== next.access_version
    ) {
      ++requestVersion.current;
      setRows([]);
      setSelected([]);
      setConversationId(null);
      setAccessEpoch((v) => v + 1);
    }
    accessVersion.current = next.access_version;
    setSpace((current) =>
      next.spaces.some((item) => item.id === current) ? current : "",
    );
    setFolder((current) =>
      next.folders.some((item) => item.id === current) ? current : "",
    );
    setData(next);
    onTeamUpdate(next.org);
    return next;
  }, [org, onTeamUpdate]);
  const loadRows = useCallback(
    async (offset = 0) => {
      const version = ++requestVersion.current;
      setLoading(true);
      try {
        const params = new URLSearchParams({
          q: query,
          space,
          folder,
          trash: String(view === "trash"),
          offset: String(offset),
        });
        const next = await team.request<TeamNoteRow[]>(
          "GET",
          orgPath(org, `/notes?${params}`),
        );
        if (requestVersion.current === version) {
          setRows((old) => (offset ? [...old, ...next] : next));
          setMore(next.length === 100);
          setError("");
        }
      } catch (e) {
        if (requestVersion.current === version) {
          setRows([]);
          setConversationId(null);
          setError(String(e));
        }
      } finally {
        if (requestVersion.current === version) setLoading(false);
      }
    },
    [org, query, space, folder, view],
  );
  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
  }, [refresh]);
  const location = JSON.stringify([space, folder, query, view]);
  const previousLocation = useRef(location);
  useEffect(() => {
    ++requestVersion.current;
    setRows([]);
    if (previousLocation.current !== location) {
      previousLocation.current = location;
      setSelected([]);
      setConversationId(null);
      setNoteId(null);
    }
    const timer = window.setTimeout(() => {
      if (view === "notes" || view === "trash") void loadRows();
    }, 180);
    return () => {
      clearTimeout(timer);
      ++requestVersion.current;
    };
  }, [loadRows, view, location]);
  useEffect(() => {
    if (loadedAccessEpoch.current === accessEpoch) return;
    loadedAccessEpoch.current = accessEpoch;
    if (view === "notes" || view === "trash") void loadRows();
  }, [accessEpoch, loadRows, view]);
  // Recheck access while the workspace is visible, including after a Mac wakes.
  useEffect(() => {
    const check = () => {
      if (document.visibilityState === "visible")
        refresh()
          .then((next) => {
            if (space && !next.spaces.some((s) => s.id === space)) {
              setSpace("");
              setFolder("");
              setNoteId(null);
              setConversationId(null);
            }
          })
          .catch((e) => {
            ++requestVersion.current;
            setData(null);
            setRows([]);
            setSelected([]);
            setNoteId(null);
            setConversationId(null);
            setError(String(e));
          });
    };
    const timer = window.setInterval(check, 30_000);
    window.addEventListener("focus", check);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", check);
    };
  }, [refresh, space]);
  const navigate = (nextSpace = "", nextFolder = "") => {
    setSpace(nextSpace);
    setFolder(nextFolder);
    setView("notes");
    setQuery("");
    setNoteId(null);
  };
  const changeSelection = (ids: string[]) => {
    setSelected(ids);
    setConversationId(null);
  };
  const currentSpace = data?.spaces.find((s) => s.id === space);
  const currentFolder = data?.folders.find((f) => f.id === folder);
  const scopeName =
    currentFolder?.name ??
    (currentSpace ? collectionName(currentSpace) : "Team meetings");
  const isAdmin = data?.org.role === "owner" || data?.org.role === "admin";
  const nestedFolders = (
    spaceId: string,
    parent: string | null = null,
    depth = 0,
  ): ReactNode =>
    data?.folders
      .filter((f) => f.space_id === spaceId && f.parent_id === parent)
      .map((f) => (
        <div key={f.id}>
          <button
            style={{ paddingLeft: `${20 + Math.min(depth, 5) * 12}px` }}
            title={f.name}
            aria-current={
              view === "notes" && folder === f.id ? "page" : undefined
            }
            className={view === "notes" && folder === f.id ? "on" : ""}
            onClick={() => navigate(spaceId, f.id)}
          >
            <Folder size={14} />
            <span>{f.name}</span>
          </button>
          {depth < 20 && nestedFolders(spaceId, f.id, depth + 1)}
        </div>
      ));
  const openMessage = async (member: string) => {
    const room = await team.request<TeamChatRoom>(
      "POST",
      orgPath(org, "/chat-rooms"),
      { kind: "direct", member_id: member },
    );
    setRequestedRoom(room);
    setView("messages");
    setNoteId(null);
  };
  const knowledgeView = ["notes", "answers", "trash"].includes(view);
  return (
    <>
      <nav className="team-primary-nav" aria-label="Team navigation">
        <button
          aria-current={knowledgeView ? "page" : undefined}
          className={knowledgeView ? "on" : ""}
          onClick={() => {
            if (!knowledgeView) setView("notes");
            else navigate();
          }}
        >
          <BookOpen size={17} /> Meetings
        </button>
        <button
          aria-current={view === "messages" ? "page" : undefined}
          className={view === "messages" ? "on" : ""}
          onClick={() => setView("messages")}
        >
          <MessageSquare size={17} /> Messages
          {unread > 0 && (
            <b aria-label={`${unread} unread messages`}>
              {unread > 99 ? "99+" : unread}
            </b>
          )}
        </button>
        <button
          aria-current={view === "search" ? "page" : undefined}
          className={view === "search" ? "on" : ""}
          onClick={() => {
            setSearchMeeting(null);
            setView("search");
          }}
        >
          <Search size={17} /> Search
        </button>
        <button
          aria-current={view === "people" ? "page" : undefined}
          className={view === "people" ? "on" : ""}
          onClick={() => setView("people")}
        >
          <Users size={17} /> People{data && <span>{data.members.length}</span>}
        </button>
        <button
          className={`team-nav-settings${view === "admin" ? " on" : ""}`}
          aria-current={view === "admin" ? "page" : undefined}
          aria-label="Team settings"
          onClick={() => setView("admin")}
        >
          <Settings size={17} />
        </button>
      </nav>
      {data && (
        <div hidden={view !== "messages"}>
          <TeamMessages
            key={`${data.org.id}:${data.user.id}`}
            data={data}
            active={view === "messages"}
            requestedRoom={requestedRoom}
            requestedMessage={requestedMessage}
            onRequestedMessageHandled={() => setRequestedMessage(null)}
            onSearch={(room) => {
              setSearchRoom(room);
              setSearchMeeting(null);
              setView("search");
            }}
            onUnread={setUnread}
            notificationTarget={notificationTarget}
            onNotificationHandled={onNotificationHandled}
          />
        </div>
      )}
      {data &&
        view === "search" &&
        (searchMeeting ? (
          <SharedMeeting
            key={searchMeeting}
            org={org}
            id={searchMeeting}
            folders={data.folders}
            accessEpoch={accessEpoch}
            backLabel="Search results"
            onBack={() => setSearchMeeting(null)}
          />
        ) : (
          <TeamSearch
            key={`${org}:${data.user.id}`}
            data={data}
            room={searchRoom}
            onRoom={setSearchRoom}
            onOpen={(hit) => {
              if (hit.kind === "meeting") setSearchMeeting(hit.id);
              else {
                setRequestedMessage({ id: hit.id, nonce: Date.now() });
                setView("messages");
              }
            }}
          />
        ))}
      {data && view === "people" && (
        <TeamPeople
          data={data}
          onMessage={openMessage}
          onManage={() => setView("admin")}
        />
      )}
      {data && view === "admin" && (
        <main className="team-settings-page">
          <TeamAdministration data={data} refresh={refresh} />
        </main>
      )}
      <div className="team-layout meetings-layout" hidden={!knowledgeView}>
        <aside
          className="team-sidebar meetings-sidebar"
          aria-label="Meeting collections"
        >
          <h2 className="meetings-sidebar-title">Library</h2>
          <button
            aria-current={view === "notes" && !space ? "page" : undefined}
            className={view === "notes" && !space ? "on" : ""}
            onClick={() => navigate()}
          >
            <BookOpen size={15} /> All meetings
          </button>
          <div className="meetings-collections">
            <details open>
              <summary>
                <ChevronDown size={13} />
                <span>Collections</span>
              </summary>
              <div className="meetings-collection-list">
                {data?.spaces
                  .slice()
                  .sort((a, b) =>
                    collectionName(a).localeCompare(collectionName(b)),
                  )
                  .map((s) => (
                    <div key={s.id} className="team-space-nav">
                      <button
                        aria-current={
                          view === "notes" && space === s.id && !folder
                            ? "page"
                            : undefined
                        }
                        title={collectionName(s)}
                        className={
                          view === "notes" && space === s.id && !folder
                            ? "on"
                            : ""
                        }
                        onClick={() => navigate(s.id)}
                      >
                        {s.visibility === "restricted" ? (
                          <Lock size={14} />
                        ) : (
                          <Users size={14} />
                        )}
                        <span className="team-collection-name">
                          {collectionName(s)}
                          <small>
                            {s.visibility === "team"
                              ? "All members"
                              : "Restricted"}
                          </small>
                        </span>
                      </button>
                      {nestedFolders(s.id)}
                    </div>
                  ))}
                {!data?.spaces.length && (
                  <p className="meetings-collection-empty">
                    Collections organize shared meetings by project or topic.
                  </p>
                )}
              </div>
            </details>
            {isAdmin && (
              <button
                className="meetings-create-collection"
                aria-label="Create collection"
                title="Create collection"
                onClick={() => setEditor("space")}
              >
                <Plus size={16} />
              </button>
            )}
          </div>
          <div className="team-sidebar-bottom">
            <button
              aria-current={view === "answers" ? "page" : undefined}
              className={view === "answers" ? "on" : ""}
              onClick={() => {
                setView("answers");
                setNoteId(null);
              }}
            >
              <BookOpen size={15} /> Saved answers
            </button>
            <button
              aria-current={view === "trash" ? "page" : undefined}
              className={view === "trash" ? "on" : ""}
              onClick={() => {
                setView("trash");
                setSpace("");
                setFolder("");
              }}
            >
              <Trash2 size={15} /> Trash
            </button>
          </div>
        </aside>
        <main className="team-main">
          {error && (
            <div className="team-error" role="alert">
              {error}
              <button
                className="team-text-button"
                onClick={() => {
                  void refresh()
                    .then(() => loadRows())
                    .catch((e) => setError(String(e)));
                }}
              >
                Retry
              </button>
            </div>
          )}
          {!data && !error && (
            <div className="team-loading">
              <Loader size={16} className="spin" /> Loading your team…
            </div>
          )}
          {data && view === "answers" && !noteId ? (
            <SavedAnswers
              key={accessEpoch}
              org={org}
              onSource={(id) => setNoteId(id)}
            />
          ) : data && noteId ? (
            <SharedMeeting
              key={noteId}
              org={org}
              id={noteId}
              folders={data.folders}
              accessEpoch={accessEpoch}
              onBack={() => {
                setNoteId(null);
                void loadRows();
              }}
            />
          ) : (
            data && (
              <>
                <header className="team-library-head">
                  <div>
                    <h1>{view === "trash" ? "Trash" : scopeName}</h1>
                    <p>
                      {view === "trash"
                        ? "Removed shared copies. Local originals remain in their owners’ libraries."
                        : currentFolder?.description ||
                          currentSpace?.description ||
                          "Shared notes, transcripts, and decisions."}
                    </p>
                    {currentSpace && (
                      <span className="team-collection-audience">
                        {currentSpace.visibility === "restricted" ? (
                          <Lock size={12} />
                        ) : (
                          <Users size={12} />
                        )}
                        {collectionAudience(currentSpace)} ·{" "}
                        {currentSpace.role === "editor"
                          ? "You can contribute"
                          : "View only"}
                      </span>
                    )}
                  </div>
                  {view === "notes" && (
                    <button
                      className="team-primary team-share-button"
                      onClick={() => setShareHelp(true)}
                    >
                      <Share2 size={14} /> Share a meeting
                    </button>
                  )}
                  <button
                    className="team-text-button"
                    aria-label="Refresh shared meetings"
                    onClick={() => {
                      void refresh()
                        .then(() => loadRows())
                        .catch((e) => setError(String(e)));
                    }}
                  >
                    <RefreshCw size={15} />
                  </button>
                  {currentSpace?.role === "editor" && view === "notes" && (
                    <button
                      className="team-text-button"
                      onClick={() => setEditor("folder")}
                    >
                      <Plus size={14} /> Folder
                    </button>
                  )}
                  {currentFolder &&
                    currentSpace?.role === "editor" &&
                    view === "notes" && (
                      <button
                        className="team-text-button"
                        onClick={() => setEditor("editFolder")}
                      >
                        Edit folder
                      </button>
                    )}
                </header>
                {view === "notes" && (
                  <TeamChat
                    key={accessEpoch}
                    org={org}
                    data={data}
                    space={space}
                    folder={folder}
                    selected={selected}
                    scopeName={scopeName}
                    id={conversationId}
                    onConversation={setConversationId}
                    onSource={setNoteId}
                  />
                )}
                <div className="team-list-tools">
                  <label>
                    <Search size={15} />
                    <input
                      type="search"
                      aria-label="Search shared notes and transcripts"
                      placeholder="Search notes and transcripts"
                      value={query}
                      maxLength={500}
                      onChange={(e) => setQuery(e.target.value)}
                    />
                  </label>
                  <span>
                    {selected.length
                      ? `${selected.length} selected`
                      : `${rows.length}${more ? "+" : ""} ${rows.length === 1 ? "meeting" : "meetings"}`}
                  </span>
                </div>
                {selected.length > 0 && (
                  <div className="team-selection">
                    <button onClick={() => changeSelection([])}>
                      Clear selection
                    </button>
                    <span>
                      Ask a question above to work with these meetings (up to
                      40).
                    </span>
                  </div>
                )}
                <div className="team-note-list" aria-busy={loading}>
                  {rows.map((row) => (
                    <div className="team-note-row" key={row.id}>
                      {view === "notes" && (
                        <input
                          type="checkbox"
                          checked={selected.includes(row.id)}
                          disabled={
                            selected.length >= 40 && !selected.includes(row.id)
                          }
                          aria-label={`Select ${row.title}`}
                          onChange={(e) =>
                            changeSelection(
                              e.target.checked
                                ? [...selected, row.id]
                                : selected.filter((id) => id !== row.id),
                            )
                          }
                        />
                      )}
                      <button onClick={() => setNoteId(row.id)}>
                        <strong>{row.title}</strong>
                        <span className="team-excerpt">
                          {row.excerpt
                            .replace(/^#+\s*/gm, "")
                            .replace(/\*\*/g, "")}
                        </span>
                        <span className="team-note-meta">
                          {row.owner_name} ·{" "}
                          {new Date(row.occurred_at).toLocaleDateString(
                            undefined,
                            { month: "short", day: "numeric" },
                          )}
                          {data.spaces.find((s) => s.id === row.space_id) && (
                            <>
                              {" "}
                              ·{" "}
                              {collectionName(
                                data.spaces.find((s) => s.id === row.space_id)!,
                              )}
                            </>
                          )}
                          {row.has_transcript ? " · Transcript included" : ""}
                        </span>
                      </button>
                      <ChevronRight size={15} aria-hidden="true" />
                    </div>
                  ))}
                  {loading && (
                    <div className="team-loading">
                      <Loader size={16} className="spin" /> Loading meetings…
                    </div>
                  )}
                  {!loading && !rows.length && (
                    <div className="team-empty">
                      <h2>
                        {query
                          ? "No shared meetings match."
                          : view === "trash"
                            ? "Nothing in Trash."
                            : "Build your team’s shared memory."}
                      </h2>
                      <p>
                        {query
                          ? "Try a name, decision, or phrase from the transcript."
                          : view === "trash"
                            ? "Removed shared meetings will appear here."
                            : "Open a completed meeting in your Library and choose Share → Publish to team. Only the content you review is published."}
                      </p>
                    </div>
                  )}
                  {more && !loading && (
                    <button
                      className="team-text-button"
                      onClick={() => void loadRows(rows.length)}
                    >
                      Load more meetings
                    </button>
                  )}
                </div>
              </>
            )
          )}
          {editor && data && (
            <CreateTeamLocation
              kind={editor}
              org={org}
              space={currentSpace}
              parent={currentFolder}
              folders={data.folders}
              onClose={() => setEditor(null)}
              onSaved={async () => {
                setEditor(null);
                await refresh();
              }}
            />
          )}
        </main>
      </div>
      {!data && !knowledgeView && (
        <p className="team-empty" role="status">
          {error || "Loading your team…"}
        </p>
      )}
      {shareHelp && (
        <TeamDialog
          title="Share a meeting with your team"
          onClose={() => setShareHelp(false)}
        >
          <div className="team-form">
            <p>
              Meetings start in your private Library. Choose which ones become
              company knowledge.
            </p>
            <ol className="team-share-steps">
              <li>
                Open a completed meeting in <strong>Library</strong>.
              </li>
              <li>
                Choose <strong>Share → Publish to team</strong>.
              </li>
              <li>Choose a collection, review who can read it, and publish.</li>
            </ol>
            <p className="team-muted">
              A shared copy appears here for everyone with access. Personal
              notes and recordings stay in your Library.
            </p>
            <button
              className="team-primary"
              onClick={() => {
                setShareHelp(false);
                onOpenLibrary?.();
              }}
            >
              {onOpenLibrary ? "Open my Library" : "Got it"}
            </button>
          </div>
        </TeamDialog>
      )}
    </>
  );
}

function CreateTeamLocation({
  kind,
  org,
  space,
  parent,
  folders,
  onClose,
  onSaved,
}: {
  kind: "space" | "folder" | "editFolder";
  org: string;
  space?: TeamSpace;
  parent?: TeamFolder;
  folders: TeamFolder[];
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const [name, setName] = useState(
      kind === "editFolder" ? (parent?.name ?? "") : "",
    ),
    [description, setDescription] = useState(
      kind === "editFolder" ? (parent?.description ?? "") : "",
    ),
    [visibility, setVisibility] = useState("restricted");
  const [parentId, setParentId] = useState(
    kind === "editFolder" ? (parent?.parent_id ?? "") : (parent?.id ?? ""),
  );
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  return (
    <TeamDialog
      title={
        kind === "space"
          ? "Create a collection"
          : kind === "editFolder"
            ? "Edit folder"
            : "Create a folder"
      }
      onClose={onClose}
    >
      <form
        className="team-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          setBusy(true);
          setError("");
          try {
            await team.request(
              kind === "editFolder" ? "PUT" : "POST",
              orgPath(
                org,
                kind === "space"
                  ? "/spaces"
                  : kind === "editFolder"
                    ? `/folders/${parent?.id}`
                    : "/folders",
              ),
              {
                name,
                description,
                visibility,
                space_id: space?.id,
                parent_id: parentId || null,
              },
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
            autoFocus
            required
            value={name}
            maxLength={200}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <label>
          Description
          <textarea
            value={description}
            maxLength={4000}
            onChange={(e) => setDescription(e.target.value)}
            rows={3}
          />
        </label>
        {kind === "space" && (
          <p className="team-muted">
            A collection groups meeting notes around a project, client, or
            topic. Choose who can read and contribute.
          </p>
        )}
        {kind === "space" && (
          <label>
            Who can access
            <select
              value={visibility}
              onChange={(e) => setVisibility(e.target.value)}
            >
              <option value="restricted">
                Admins and invited members or groups
              </option>
              <option value="team">All team members</option>
            </select>
          </label>
        )}
        {kind !== "space" && (
          <>
            <label>
              Parent folder
              <select
                value={parentId}
                onChange={(e) => setParentId(e.target.value)}
              >
                <option value="">{space?.name} (top level)</option>
                {folders
                  .filter(
                    (f) =>
                      f.space_id === space?.id &&
                      (kind !== "editFolder" || f.id !== parent?.id),
                  )
                  .map((f) => (
                    <option key={f.id} value={f.id}>
                      {f.name}
                    </option>
                  ))}
              </select>
            </label>
            <p className="team-muted">
              Folders use the same access as their collection.
            </p>
          </>
        )}
        {error && (
          <p className="team-error" role="alert">
            {error}
          </p>
        )}
        <button className="team-primary" disabled={busy}>
          {kind === "editFolder" ? "Save folder" : `Create ${kind}`}
        </button>
      </form>
    </TeamDialog>
  );
}

function SharedMeeting({
  backLabel = "Shared meetings",
  org,
  id,
  folders,
  accessEpoch,
  onBack,
}: {
  org: string;
  id: string;
  folders: TeamFolder[];
  accessEpoch: number;
  backLabel?: string;
  onBack: () => void;
}) {
  const [note, setNote] = useState<TeamNote | null>(null),
    [error, setError] = useState("");
  const [editing, setEditing] = useState(false),
    [title, setTitle] = useState(""),
    [summary, setSummary] = useState("");
  const [busy, setBusy] = useState(false),
    [copied, setCopied] = useState(false);
  const [editRevision, setEditRevision] = useState<number | null>(null);
  const [editFolders, setEditFolders] = useState<string[]>([]);
  const transcriptPanel = useRef<HTMLDetailsElement>(null);
  const transcriptRows = useRef<(HTMLParagraphElement | null)[]>([]);
  const [highlight, setHighlight] = useState(-1);
  const openSource = (source: string) => {
    if (!note?.transcript || source.toLowerCase() === "notes") {
      setError("This source was not included in the published meeting.");
      return;
    }
    const seconds = (value: string) => {
      const match = value.match(/(\d+):(\d{2})/);
      return match ? Number(match[1]) * 60 + Number(match[2]) : -1;
    };
    const target = seconds(source),
      lines = note.transcript.split("\n");
    let closest = -1,
      distance = Infinity;
    lines.forEach((line, i) => {
      const time = seconds(line);
      if (time >= 0 && Math.abs(time - target) < distance) {
        closest = i;
        distance = Math.abs(time - target);
      }
    });
    if (closest < 0) {
      setError("No timestamped transcript passage is available.");
      return;
    }
    if (transcriptPanel.current) transcriptPanel.current.open = true;
    setHighlight(closest);
    setError("");
    requestAnimationFrame(() =>
      transcriptRows.current[closest]?.scrollIntoView({ block: "center" }),
    );
  };
  const load = useCallback(async () => {
    const n = await team.request<TeamNote>("GET", orgPath(org, `/notes/${id}`));
    setNote(n);
    if (!n.can_edit) setEditing(false);
    return n;
  }, [org, id]);
  useEffect(() => {
    let active = true;
    team
      .request<TeamNote>("GET", orgPath(org, `/notes/${id}`))
      .then((n) => {
        if (active) {
          setNote(n);
          if (!n.can_edit) setEditing(false);
        }
      })
      .catch((e) => {
        if (active) {
          setNote(null);
          setEditing(false);
          setError(String(e));
        }
      });
    return () => {
      active = false;
    };
  }, [org, id, accessEpoch]);
  useEffect(() => {
    const timer = window.setInterval(() => {
      load().catch((e) => {
        setNote(null);
        setError(String(e));
      });
    }, 30_000);
    return () => clearInterval(timer);
  }, [load]);
  return (
    <article className="team-shared-note">
      <button
        className="team-text-button"
        onClick={() => {
          if (
            !editing ||
            confirm("Discard unsaved changes to this shared meeting?")
          )
            onBack();
        }}
      >
        <ArrowLeft size={15} /> {backLabel}
      </button>
      {error && (
        <p className="team-error" role="alert">
          {error}
          <button
            className="team-text-button"
            onClick={() =>
              void load()
                .then(() => setError(""))
                .catch((e) => setError(String(e)))
            }
          >
            Reload
          </button>
        </p>
      )}
      {!note && !error && <p>Loading meeting…</p>}
      {note && (
        <>
          <header>
            <h1>{note.title}</h1>
            <p className="team-muted">
              Shared by {note.owner_name} ·{" "}
              {new Date(note.occurred_at).toLocaleDateString()} · Revision{" "}
              {note.revision}
            </p>
          </header>
          <div className="team-note-actions">
            <button
              onClick={async () => {
                try {
                  await copyTeamText(
                    `# ${note.title}\n\n${note.summary}${note.transcript ? `\n\n## Transcript\n${note.transcript}` : ""}`,
                  );
                  setCopied(true);
                } catch (e) {
                  setError(String(e));
                }
              }}
            >
              {copied ? <Check size={14} /> : <Copy size={14} />}
              {copied ? "Copied" : "Copy Markdown"}
            </button>
            {note.can_edit && !note.trashed_at && !editing && (
              <button
                onClick={() => {
                  setTitle(note.title);
                  setSummary(note.summary);
                  setEditRevision(note.revision);
                  setEditFolders(note.folder_ids);
                  setEditing(true);
                }}
              >
                Edit shared notes
              </button>
            )}
            {note.can_manage && (
              <button
                disabled={busy}
                onClick={async () => {
                  if (
                    !note.trashed_at &&
                    !confirm(
                      "Move this shared copy to team Trash? Your local meeting is kept.",
                    )
                  )
                    return;
                  setBusy(true);
                  try {
                    await team.request(
                      note.trashed_at ? "POST" : "DELETE",
                      orgPath(
                        org,
                        `/notes/${id}${note.trashed_at ? "/restore" : ""}`,
                      ),
                      { revision: note.revision },
                    );
                    onBack();
                  } catch (e) {
                    setError(String(e));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {note.trashed_at ? "Restore" : "Move to Trash"}
              </button>
            )}
          </div>
          {editing ? (
            <TeamDialog
              title="Edit shared meeting"
              busy={busy}
              onClose={() => {
                if (confirm("Discard unsaved changes to this shared meeting?"))
                  setEditing(false);
              }}
            >
              {error && (
                <p className="team-error" role="alert">
                  {error}
                </p>
              )}
              <form
                className="team-form"
                onSubmit={async (e) => {
                  e.preventDefault();
                  if (busy) return;
                  setBusy(true);
                  try {
                    const n = await team.request<TeamNote>(
                      "PATCH",
                      orgPath(org, `/notes/${id}`),
                      {
                        title,
                        summary,
                        folder_ids: editFolders,
                        revision: editRevision,
                      },
                    );
                    setNote(n);
                    setEditing(false);
                    setError("");
                  } catch (e) {
                    setError(String(e));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                <label>
                  Title
                  <input
                    value={title}
                    required
                    maxLength={500}
                    onChange={(e) => setTitle(e.target.value)}
                  />
                </label>
                <label>
                  Shared notes
                  <textarea
                    value={summary}
                    required
                    rows={18}
                    maxLength={300000}
                    onChange={(e) => setSummary(e.target.value)}
                  />
                </label>
                <fieldset>
                  <legend>Folders</legend>
                  {folders
                    .filter((f) => f.space_id === note.space_id)
                    .map((f) => (
                      <label key={f.id} className="team-checkbox">
                        <input
                          type="checkbox"
                          checked={editFolders.includes(f.id)}
                          onChange={(e) =>
                            setEditFolders((old) =>
                              e.target.checked
                                ? [...old, f.id]
                                : old.filter((id) => id !== f.id),
                            )
                          }
                        />
                        {f.name}
                      </label>
                    ))}
                </fieldset>
                <div className="team-form-actions">
                  <button className="team-primary" disabled={busy}>
                    Save shared notes
                  </button>
                  <button
                    type="button"
                    className="team-text-button"
                    onClick={() => setEditing(false)}
                  >
                    Cancel
                  </button>
                </div>
              </form>
            </TeamDialog>
          ) : (
            <MdBlock md={note.summary} onSource={openSource} />
          )}
          {!!note.transcript && (
            <details ref={transcriptPanel} className="team-transcript">
              <summary>Full transcript</summary>
              <div className="team-transcript-lines">
                {note.transcript.split("\n").map((line, i) => (
                  <p
                    key={i}
                    ref={(node) => {
                      transcriptRows.current[i] = node;
                    }}
                    className={highlight === i ? "team-source-highlight" : ""}
                  >
                    {line}
                  </p>
                ))}
              </div>
            </details>
          )}
          <p className="team-muted team-note-provenance">
            This is a shared copy. Changes here do not overwrite the publisher’s
            local meeting.
          </p>
        </>
      )}
    </article>
  );
}
