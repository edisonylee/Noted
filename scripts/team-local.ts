// Install a persistent, loopback-only team service and connect the standard app.
// No private meeting or recording is copied into the organizational database.
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  writeFileSync,
  copyFileSync,
} from "node:fs";
import { createHash, randomBytes } from "node:crypto";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { TeamStore } from "../services/team/store";
import { ensureExampleWorkspace } from "../services/team/examples";

if (process.platform !== "darwin")
  throw new Error("Local team installation currently supports macOS.");
const { values } = parseArgs({
  args: process.argv.slice(2),
  options: {
    owner: { type: "string" },
    workspace: { type: "string" },
    examples: { type: "boolean", default: false },
  },
});
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const support = join(homedir(), "Library", "Application Support");
const state = join(support, "Noted Team");
const database = join(state, "team.sqlite");
const identityPath = join(state, "local-owner.json");
const connectionPath = join(support, "com.noted.app", "team-connection.json");
const server = "http://127.0.0.1:8790";
const label = "com.noted.team";
const service = `gui/${process.getuid!()}/${label}`;
const plist = join(homedir(), "Library", "LaunchAgents", `${label}.plist`);
const bun = Bun.which("bun") ?? process.execPath;
const run = (args: string[], allowFailure = false) => {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode && !allowFailure)
    throw new Error(
      `${args[0]} failed (${result.exitCode}): ${result.stderr.toString().trim()}`,
    );
  return result;
};
const atomicJson = (path: string, value: unknown) => {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  writeFileSync(`${path}.tmp`, JSON.stringify(value, null, 2), { mode: 0o600 });
  chmodSync(`${path}.tmp`, 0o600);
  renameSync(`${path}.tmp`, path);
};
if (
  existsSync(connectionPath) &&
  JSON.parse(readFileSync(connectionPath, "utf8")).server !== server
)
  throw new Error(
    "Noted is already connected to another server. Disconnect it in the app before setting up a local workspace.",
  );
if (
  !existsSync(plist) &&
  run(["lsof", "-t", "-iTCP:8790", "-sTCP:LISTEN"], true)
    .stdout.toString()
    .trim()
)
  throw new Error(
    "Port 8790 is already in use. Stop the temporary team preview before installing the persistent service.",
  );

mkdirSync(state, { recursive: true, mode: 0o700 });
chmodSync(state, 0o700);
const setupKey = randomBytes(32).toString("base64url");
const store = new TeamStore(database, setupKey);
let identity: { user: string; org: string };
if (store.get("SELECT id FROM organizations LIMIT 1")) {
  if (!existsSync(identityPath))
    throw new Error(
      "The team database exists but its local owner record is missing. Restore local-owner.json from your backup.",
    );
  identity = JSON.parse(readFileSync(identityPath, "utf8"));
  if (store.role(identity.user, identity.org) !== "owner")
    throw new Error(
      "Local setup cannot replace a transferred workspace owner. Use the app's current account.",
    );
} else {
  if (!values.owner?.trim() || !values.workspace?.trim())
    throw new Error("First setup needs --owner and --workspace names.");
  const setup = store.bootstrap(setupKey, values.workspace, values.owner);
  identity = { user: store.authenticate(setup.token), org: setup.org };
  store.signout(setup.token);
  atomicJson(identityPath, identity);
}
if (values.examples) ensureExampleWorkspace(store, identity.user);
const token = store.session(identity.user);
const organizations = store.orgs(identity.user).map((org) => org.name);
store.db.close();
chmodSync(database, 0o600);

const source = join(root, "services", "team");
const version = createHash("sha256");
for (const file of ["server.ts", "store.ts", "schema.sql"])
  version.update(readFileSync(join(source, file)));
const runtime = join(state, "runtime", version.digest("hex").slice(0, 16));
mkdirSync(runtime, { recursive: true, mode: 0o700 });
const build = run([
  bun,
  "build",
  join(source, "server.ts"),
  "--target=bun",
  "--outfile",
  join(runtime, "server.js.next"),
]);
copyFileSync(join(source, "schema.sql"), join(runtime, "schema.sql"));
renameSync(join(runtime, "server.js.next"), join(runtime, "server.js"));
void build;
const logs = join(state, "logs");
mkdirSync(logs, { recursive: true, mode: 0o700 });
const xml = (value: string) =>
  value.replace(
    /[&<>"']/g,
    (c) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&apos;",
      })[c]!,
  );
const definition = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>${label}</string>
<key>ProgramArguments</key><array><string>${xml(bun)}</string><string>${xml(join(runtime, "server.js"))}</string></array>
<key>WorkingDirectory</key><string>${xml(state)}</string>
<key>EnvironmentVariables</key><dict><key>NOTED_TEAM_DB</key><string>${xml(database)}</string><key>NOTED_TEAM_HOST</key><string>127.0.0.1</string><key>NOTED_TEAM_PORT</key><string>8790</string></dict>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer>
<key>ProcessType</key><string>Background</string><key>Umask</key><integer>63</integer>
<key>StandardOutPath</key><string>${xml(join(logs, "service.log"))}</string>
<key>StandardErrorPath</key><string>${xml(join(logs, "error.log"))}</string>
</dict></plist>`;
mkdirSync(dirname(plist), { recursive: true });
writeFileSync(`${plist}.next`, definition, { mode: 0o600 });
run(["plutil", "-lint", `${plist}.next`]);
if (existsSync(plist)) run(["launchctl", "bootout", service], true);
renameSync(`${plist}.next`, plist);
run(["launchctl", "bootstrap", `gui/${process.getuid!()}`, plist]);
run(["launchctl", "kickstart", service]);

let ready = false;
for (let attempt = 0; attempt < 30; attempt++) {
  try {
    const response = await fetch(`${server}/v1/orgs`, {
      headers: { Authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(1000),
    });
    if (
      response.ok &&
      (await response.json()).some(
        (org: { id: string }) => org.id === identity.org,
      )
    ) {
      ready = true;
      break;
    }
  } catch {
    /* launchd may still be starting the listener */
  }
  await Bun.sleep(300);
}
if (!ready)
  throw new Error(
    `The service did not become ready. Check ${join(logs, "error.log")}. The app connection was not changed.`,
  );

const account = `team_session:${createHash("sha256").update(server).digest("hex")}`;
const previous = run(
  [
    "security",
    "find-generic-password",
    "-s",
    "com.noted.app",
    "-a",
    account,
    "-w",
  ],
  true,
);
// Match the app's Keychain contract; credentials are never printed or written
// into the LaunchAgent, connection JSON, source checkout, or plaintext files.
run([
  "security",
  "add-generic-password",
  "-U",
  "-s",
  "com.noted.app",
  "-a",
  account,
  "-w",
  token,
]);
atomicJson(connectionPath, { server });
if (!previous.exitCode && previous.stdout.toString().trim()) {
  await fetch(`${server}/v1/session`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${previous.stdout.toString().trim()}` },
    signal: AbortSignal.timeout(3000),
  }).catch(() => {});
}
console.log(`Noted is connected to ${server}`);
console.log(`Workspaces: ${organizations.join(", ")}`);
console.log(
  "The service starts at login and restarts automatically. Only this Mac can connect.",
);
console.log(`Persistent data: ${state}`);
console.log(
  "Open Home, then Team in Noted to refresh the connection. Account sessions expire after 30 days; rerun this command to reconnect the local owner.",
);
