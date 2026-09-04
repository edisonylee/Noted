import { api } from "../api";
import type { TeamOrg, TeamAnswer, TeamConversation } from "./types";

// This transport exists only in the dedicated, loopback-only development preview.
// Product builds always use the native Keychain-backed command surface.
const preview =
  import.meta.env.DEV && window.location.pathname === "/team-preview.html";
let previewToken = "";
async function previewRequest<T>(
  method: string,
  path: string,
  body?: unknown,
  token = previewToken,
): Promise<T> {
  const response = await fetch(`/__team${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: body == null ? undefined : JSON.stringify(body),
  });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error || "Team service unavailable");
  return value as T;
}
export const team = {
  status: () =>
    preview
      ? Promise.resolve({
          connected: !!previewToken,
          server: "http://127.0.0.1:8790",
        })
      : api.teamStatus(),
  connect: async (
    server: string,
    mode: string,
    secret: string,
    organization = "",
    name = "",
  ): Promise<TeamOrg[]> => {
    if (!preview)
      return api.teamConnect(server, mode, secret, organization, name);
    if (!/^http:\/\/(127\.0\.0\.1|localhost):8790\/?$/.test(server))
      throw new Error(
        "The development preview connects only to the local team service on port 8790",
      );
    const session =
      mode === "signin"
        ? { token: secret }
        : mode === "create"
          ? await previewRequest<{ token: string }>(
              "POST",
              "/v1/bootstrap",
              { organization, name },
              secret,
            )
          : await previewRequest<{ token: string }>(
              "POST",
              "/v1/accept",
              { invitation: secret },
              "",
            );
    const orgs = await previewRequest<TeamOrg[]>(
      "GET",
      "/v1/orgs",
      undefined,
      session.token,
    );
    previewToken = session.token;
    return orgs;
  },
  disconnect: async () => {
    if (!preview) return api.teamDisconnect();
    try {
      await previewRequest("DELETE", "/v1/session");
    } finally {
      previewToken = "";
    }
  },
  request: <T>(method: string, path: string, body?: unknown): Promise<T> =>
    preview
      ? previewRequest<T>(method, path, body)
      : api.teamRequest<T>(method, path, body),
  ask: async (org: string, body: unknown): Promise<TeamAnswer> => {
    if (!preview) return api.teamAsk(org, body);
    // The preview exercises real authorized retrieval. Generative answers run
    // through the desktop's configured model, never a browser-supplied API key.
    const packet = await previewRequest<{
      sources: TeamAnswer["sources"];
      limited: boolean;
      conversation_revision: number;
    }>("POST", `/v1/orgs/${org}/context`, body);
    const answer = packet.sources.length
      ? "Source retrieval is working. Open this workspace in the desktop app to generate an answer with your configured model."
      : "There are no shared meetings in this scope yet.";
    const conversation = packet.sources.length
      ? await previewRequest<TeamConversation>(
          "POST",
          `/v1/orgs/${org}/conversations`,
          {
            ...(body as Record<string, unknown>),
            answer,
            sources: packet.sources,
            expected_revision: packet.conversation_revision,
          },
        )
      : undefined;
    return { ...packet, answer, conversation };
  },
};
export const orgPath = (org: string, path = "") =>
  `/v1/orgs/${encodeURIComponent(org)}${path}`;
export async function copyTeamText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const el = document.createElement("textarea");
    const focus = document.activeElement;
    el.value = text;
    el.style.position = "fixed";
    el.style.opacity = "0";
    document.body.append(el);
    el.select();
    const ok = document.execCommand("copy");
    el.remove();
    if (focus instanceof HTMLElement) focus.focus();
    if (!ok) throw new Error("Copy failed. Select and copy the text instead.");
  }
}
