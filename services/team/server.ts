import { TeamStore, TeamError, text } from "./store";

type Body = Record<string, unknown>;
export function createHandler(store: TeamStore, allowedOrigins: string[] = []) {
  const attempts = new Map<string, { count: number; reset: number }>();
  return async (request: Request, address = "local"): Promise<Response> => {
    const headers = {
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "no-referrer",
    };
    const respond = (value: unknown, status = 200) =>
      Response.json(value ?? {}, { status, headers });
    try {
      const origin = request.headers.get("Origin");
      if (origin && !allowedOrigins.includes(origin))
        throw new TeamError(403, "Origin not allowed");
      const url = new URL(request.url),
        path = url.pathname.split("/").filter(Boolean);
      const method = request.method;
      if (method === "GET" && url.pathname === "/health")
        return respond({ ok: true, service: "noted-team", version: 1 });
      if (path[0] !== "v1") throw new TeamError(404, "Not found");
      const token =
        request.headers.get("Authorization")?.replace(/^Bearer /, "") ?? "";
      const authentication = path[1] === "bootstrap" || path[1] === "accept";
      const key = authentication
        ? `auth:${address}`
        : `user:${token.slice(0, 80)}`;
      const at = Date.now();
      if (attempts.size > 10_000)
        for (const [k, v] of attempts) if (v.reset <= at) attempts.delete(k);
      if (!attempts.has(key) && attempts.size > 20_000)
        throw new TeamError(429, "Try again shortly");
      const bucket = attempts.get(key);
      if (bucket && bucket.reset > at) {
        if (++bucket.count > (authentication ? 20 : 300))
          throw new TeamError(429, "Too many requests. Try again in a minute.");
      } else attempts.set(key, { count: 1, reset: at + 60_000 });
      let body: Body = {};
      if (["POST", "PATCH", "PUT", "DELETE"].includes(method)) {
        const messageUpload =
          method === "POST" &&
          /^\/v1\/orgs\/[^/]+\/chat-rooms\/[^/]+\/messages$/.test(url.pathname);
        const limit = messageUpload ? 7_100_000 : 1_500_000;
        if (messageUpload) store.authenticate(token);
        if (Number(request.headers.get("Content-Length") ?? 0) > limit)
          throw new TeamError(413, "Request too large");
        const chunks: Uint8Array[] = [];
        let length = 0;
        const reader = request.body?.getReader();
        if (reader) {
          while (true) {
            const next = await reader.read();
            if (next.done) break;
            length += next.value.byteLength;
            if (length > limit) {
              await reader.cancel();
              throw new TeamError(413, "Request too large");
            }
            chunks.push(next.value);
          }
        }
        const bytes = new Uint8Array(length);
        let offset = 0;
        for (const chunk of chunks) {
          bytes.set(chunk, offset);
          offset += chunk.length;
        }
        if (bytes.byteLength) {
          try {
            body = JSON.parse(new TextDecoder().decode(bytes));
          } catch {
            throw new TeamError(400, "Invalid JSON");
          }
          if (!body || Array.isArray(body) || typeof body !== "object")
            throw new TeamError(400, "Expected a JSON object");
        }
      }
      if (path.length === 2 && path[1] === "bootstrap" && method === "POST")
        return respond(
          store.bootstrap(token, body.organization, body.name),
          201,
        );
      if (path.length === 2 && path[1] === "accept" && method === "POST")
        return respond(
          store.accept(
            body.invitation,
            token ? store.authenticate(token) : undefined,
          ),
        );
      if (path[1] === "api") {
        const access = store.authenticateIntegration(token);
        if (method !== "GET")
          throw new TeamError(405, "Integrations are read-only");
        if (path.length < 3 || path.length > 4)
          throw new TeamError(404, "Not found");
        return respond(
          store.integrationRead(access, path[2], path[3], url.searchParams),
        );
      }
      const user = store.authenticate(token);
      if (path.length === 2 && path[1] === "session" && method === "DELETE") {
        store.signout(token);
        return respond({});
      }
      if (path.length === 2 && path[1] === "profile") {
        if (method === "GET") return respond(store.profile(user));
        if (method === "PATCH") return respond(store.updateProfile(user, body));
      }
      if (path[1] !== "orgs") throw new TeamError(404, "Not found");
      if (path.length === 2) {
        if (method === "GET") return respond(store.orgs(user));
        if (method === "POST")
          return respond({ id: store.createOrg(user, body.name) }, 201);
      }
      const org = path[2];
      if (path.length === 3 && org === "join" && method === "POST")
        return respond(store.accept(body.invitation, user));
      if (!org) throw new TeamError(404, "Workspace not found");
      store.role(user, org);
      const resource = path[3],
        id = path[4],
        action = path[5];
      if (
        path.length > 6 ||
        (action &&
          !(
            (resource === "notes" &&
              ["restore", "share-targets"].includes(action)) ||
            (resource === "spaces" && action === "grants") ||
            (resource === "chat-rooms" &&
              ["messages", "read", "unread", "notifications", "pins"].includes(
                action,
              )) ||
            (resource === "chat-messages" &&
              ["reactions", "pin", "source", "saved"].includes(action)) ||
            (resource === "mentions" && action === "read") ||
            (resource === "profiles" && action === "avatar")
          ))
      )
        throw new TeamError(404, "Not found");
      if (
        id &&
        ["owner", "context", "activity", "access-keys"].includes(resource)
      )
        throw new TeamError(404, "Not found");
      if (path.length === 3 && method === "GET")
        return respond(store.snapshot(user, org));
      if (path.length === 3 && method === "PATCH")
        return respond(store.renameOrg(user, org, body.name));
      if (resource === "attachments" && id && !action && method === "GET")
        return respond(store.attachment(user, org, id));
      if (
        resource === "notes" &&
        id &&
        action === "share-targets" &&
        method === "GET"
      )
        return respond(store.meetingShareTargets(user, org, id));
      if (
        resource === "chat-messages" &&
        id &&
        action === "source" &&
        method === "GET"
      )
        return respond(store.meetingReference(user, org, id));
      if (resource === "saved-messages" && !id && method === "GET")
        return respond(store.savedMessages(user, org, url.searchParams));
      if (
        resource === "chat-messages" &&
        id &&
        action === "saved" &&
        method === "PUT"
      )
        return respond(store.saveMessage(user, org, id, body.active));
      if (resource === "search" && !id && method === "GET")
        return respond(store.search(user, org, url.searchParams));
      if (
        resource === "mentions" &&
        id &&
        action === "read" &&
        method === "POST"
      )
        return respond(store.readMention(user, org, id));
      if (resource === "mentions" && !id && method === "GET")
        return respond(store.mentions(user, org, url.searchParams));
      if (
        resource === "chat-messages" &&
        id &&
        action === "pin" &&
        method === "PUT"
      )
        return respond(store.pinMessage(user, org, id, body.active));
      if (resource === "chat-rooms") {
        if (id && action === "pins" && method === "GET")
          return respond(store.pinnedMessages(user, org, id));
        if (id && action === "unread" && method === "POST")
          return respond(store.markChatUnread(user, org, id, body.message_id));
        if (id && action === "notifications" && method === "PUT")
          return respond(
            store.setConversationNotifications(user, org, id, body),
          );
        if (!id && method === "GET") return respond(store.chatRooms(user, org));
        if (!id && method === "POST")
          return respond(store.createChatRoom(user, org, body), 201);
        if (id && !action && method === "GET")
          return respond(store.chatRoom(user, org, id));
        if (id && !action && method === "PATCH")
          return respond(store.updateChatRoom(user, org, id, body));
        if (id && action === "messages" && method === "GET") {
          const wait = Number(url.searchParams.get("wait") ?? 0);
          if (
            !Number.isInteger(wait) ||
            wait < 0 ||
            wait > 20_000 ||
            (wait && !url.searchParams.has("after"))
          )
            throw new TeamError(400, "Invalid wait duration");
          const page = store.chatMessages(user, org, id, url.searchParams);
          if (
            wait &&
            !page.has_more &&
            page.cursor === Number(url.searchParams.get("after"))
          ) {
            await store.waitForChat(id, page.cursor, request.signal, wait);
            store.authenticate(token);
            return respond(store.chatMessages(user, org, id, url.searchParams));
          }
          return respond(page);
        }
        if (id && action === "messages" && method === "POST")
          return respond(store.sendChatMessage(user, org, id, body), 201);
        if (id && action === "read" && method === "POST")
          return respond(
            store.readChat(
              user,
              org,
              id,
              body.cursor,
              body.version,
              body.resume === true,
            ),
          );
      }
      if (
        resource === "profiles" &&
        id &&
        action === "avatar" &&
        method === "GET"
      )
        return respond(store.profileAvatar(user, org, id));
      if (
        resource === "chat-messages" &&
        id &&
        action === "reactions" &&
        method === "PUT"
      )
        return respond(store.reactToMessage(user, org, id, body));
      if (resource === "chat-messages" && id && !action) {
        if (method === "GET")
          return respond(store.messageLocation(user, org, id));
        if (method === "PATCH")
          return respond(store.changeChatMessage(user, org, id, body));
        if (method === "DELETE")
          return respond(store.changeChatMessage(user, org, id, body, true));
      }
      if (resource === "access-keys" && method === "POST" && !id) {
        store.audit(org, user, "session.created", user);
        return respond({ token: store.session(user) }, 201);
      }
      if (resource === "invites") {
        if (!id && method === "GET")
          return respond(store.invitations(user, org));
        if (!id && method === "POST")
          return respond(store.invite(user, org, body), 201);
        if (id && method === "DELETE") {
          store.revokeInvite(user, org, id);
          return respond({});
        }
      }
      if (resource === "members" && id && method === "PATCH") {
        store.changeMember(user, org, id, body.role);
        return respond({});
      }
      if (resource === "owner" && method === "POST") {
        store.transferOwner(user, org, text(body.user_id, "member", 100));
        return respond({});
      }
      if (resource === "spaces") {
        if (!id && method === "POST")
          return respond(store.createSpace(user, org, body), 201);
        if (id && !action && method === "PATCH")
          return respond(store.updateSpace(user, org, id, body));
        if (id && action === "grants" && method === "GET")
          return respond(store.grants(user, org, id));
        if (id && action === "grants" && method === "PUT")
          return respond(store.grant(user, org, id, body));
      }
      if (resource === "groups") {
        if (!id && method === "GET") return respond(store.groups(user, org));
        if (!id && method === "POST")
          return respond(store.saveGroup(user, org, body), 201);
        if (id && method === "PUT")
          return respond(store.saveGroup(user, org, body, id));
        if (id && method === "DELETE") {
          store.deleteGroup(user, org, id);
          return respond({});
        }
      }
      if (resource === "folders") {
        if (!id && method === "POST")
          return respond(store.saveFolder(user, org, body), 201);
        if (id && method === "PUT")
          return respond(store.saveFolder(user, org, body, id));
      }
      if (resource === "notes") {
        if (!id && method === "GET") {
          const offset = Number(url.searchParams.get("offset") ?? 0);
          if (!Number.isSafeInteger(offset) || offset < 0 || offset > 100_000)
            throw new TeamError(400, "Invalid offset");
          return respond(
            store.listNotes(
              user,
              org,
              text(url.searchParams.get("q") ?? "", "query", 500, true),
              url.searchParams.get("space") ?? "",
              url.searchParams.get("folder") ?? "",
              url.searchParams.get("trash") === "true",
              100,
              offset,
            ),
          );
        }
        if (!id && method === "POST")
          return respond(store.publish(user, org, body), 201);
        if (id && !action && method === "GET")
          return respond(store.note(user, org, id));
        if (id && !action && method === "PATCH")
          return respond(store.updateNote(user, org, id, body));
        if (id && !action && method === "DELETE") {
          store.trash(user, org, id, body.revision);
          return respond({});
        }
        if (id && action === "restore" && method === "POST") {
          store.trash(user, org, id, body.revision, true);
          return respond({});
        }
      }
      if (resource === "context" && method === "POST")
        return respond(store.context(user, org, body));
      if (resource === "conversations") {
        if (!id && method === "GET")
          return respond(
            store.conversations(
              user,
              org,
              Number(url.searchParams.get("offset") ?? 0),
            ),
          );
        if (!id && method === "POST")
          return respond(store.appendConversation(user, org, body), 201);
        if (id && method === "GET")
          return respond(store.conversation(user, org, id));
        if (id && method === "DELETE") {
          store.deleteConversation(user, org, id);
          return respond({});
        }
      }
      if (resource === "answers") {
        if (!id && method === "GET") return respond(store.answers(user, org));
        if (!id && method === "POST")
          return respond(store.saveAnswer(user, org, body), 201);
        if (id && method === "GET") return respond(store.answer(user, org, id));
        if (id && method === "DELETE") {
          store.deleteAnswer(user, org, id);
          return respond({});
        }
      }
      if (resource === "integrations") {
        if (!id && method === "GET")
          return respond(store.integrationKeys(user, org));
        if (!id && method === "POST")
          return respond(store.createIntegrationKey(user, org, body), 201);
        if (id && method === "DELETE") {
          store.revokeIntegrationKey(user, org, id);
          return respond({});
        }
      }
      if (resource === "recipes") {
        if (!id && method === "POST")
          return respond(store.saveRecipe(user, org, body), 201);
        if (id && method === "PUT")
          return respond(store.saveRecipe(user, org, body, id));
        if (id && method === "DELETE") {
          store.deleteRecipe(user, org, id);
          return respond({});
        }
      }
      if (resource === "activity" && method === "GET")
        return respond(store.activity(user, org));
      throw new TeamError(404, "Not found");
    } catch (error) {
      if (error instanceof TeamError)
        return respond({ error: error.message }, error.status);
      console.error(
        "Team request failed:",
        error instanceof Error ? error.name : "unknown error",
      );
      return respond(
        { error: "The team service could not complete this request" },
        500,
      );
    }
  };
}

export function openServiceStore(path: string, setupKey = "") {
  if (setupKey && setupKey.length < 32)
    throw new Error(
      "The initial setup key must contain at least 32 characters",
    );
  const store = new TeamStore(path, setupKey);
  if (!setupKey && !store.get("SELECT id FROM organizations LIMIT 1")) {
    store.db.close();
    throw new Error(
      "A new server needs NOTED_TEAM_SETUP_KEY. An initialized server can run without it.",
    );
  }
  return store;
}

if (import.meta.main) {
  const store = openServiceStore(
    process.env.NOTED_TEAM_DB ?? "team.sqlite",
    process.env.NOTED_TEAM_SETUP_KEY ?? "",
  );
  const handler = createHandler(
    store,
    (process.env.NOTED_TEAM_ORIGINS ?? "").split(",").filter(Boolean),
  );
  const server = Bun.serve({
    hostname: process.env.NOTED_TEAM_HOST ?? "127.0.0.1",
    port: Number(process.env.NOTED_TEAM_PORT ?? process.env.PORT ?? 8790),
    maxRequestBodySize: 7_100_000,
    idleTimeout: 30,
    fetch: (request, server) =>
      handler(request, server.requestIP(request)?.address ?? "unknown"),
  });
  console.log(`Noted team service listening on ${server.url}`);
  const stop = () => {
    server.stop(true);
    store.db.close();
    process.exit(0);
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
}
