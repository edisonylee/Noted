// Join a meeting on the calendar account the event belongs to. Google Meet
// honors `?authuser=<email>` to pick among the accounts signed into the
// browser — without it, the browser's default (often a personal) account
// joins a work meeting. Constraint: the account must already be signed into
// the browser; if it isn't (or lives in a different browser profile), Google
// shows its account chooser instead — a browser boundary we can't cross from
// a URL. Non-Google links (Zoom, Teams) pass through untouched.
export function joinUrl(link: string, account?: string | null): string {
  if (!account) return link;
  try {
    const u = new URL(link);
    const host = u.hostname.toLowerCase();
    if (host === "meet.google.com" || host.endsWith(".google.com")) {
      u.searchParams.set("authuser", account);
      return u.toString();
    }
  } catch {
    // not a parseable URL — leave it alone
  }
  return link;
}
