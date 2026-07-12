// Shared between the Ask page and the mobile chat sheet: how a chat proposal
// reads, and what confirming it actually does. Proposals never touch data
// until the user confirms — this is the apply side.

import { api, type ChatProposal } from "./api";

export function proposalText(p: ChatProposal): string {
  if (p.action === "create_category") {
    return p.already_exists
      ? `“${p.name}” already exists — open it anyway?`
      : `Create a new category “${p.name}”?`;
  }
  if (p.action === "create_event") {
    const when = p.start ? `${p.date} ${p.start}${p.end ? `–${p.end}` : ""}` : `${p.date} (all day)`;
    const extras = [
      p.guests.length ? `invite ${p.guests.join(", ")}` : "",
      p.meet ? "with a Meet link" : "",
    ]
      .filter(Boolean)
      .join(", ");
    return `Put “${p.title}” on your calendar — ${when}${extras ? ` (${extras})` : ""}?`;
  }
  if (p.action === "apply_theme") {
    return `Preview “${p.theme_name}” across Noted? ${p.summary}`;
  }
  return `Apply this change — ${p.summary}?`;
}

/// Apply a confirmed proposal; returns the human confirmation line.
export async function applyProposal(p: ChatProposal): Promise<string> {
  if (p.action === "create_category") {
    await api.createCategory(p.name, p.description);
    return `Created category “${p.name}”.`;
  }
  if (p.action === "edit_entry") {
    await api.updateEntry(p.entry_id, p.data);
    return "Done — updated that entry.";
  }
  if (p.action === "apply_theme") {
    throw new Error("Theme proposals must be applied from the active theme preview.");
  }
  // create_event → the sync account's primary calendar (same home as the
  // pushed daily schedule); no account/calendar picker in a chat flow.
  const st = await api.gcalAuthStatus();
  const acct =
    st.accounts.find((a) => a.email === st.sync_account && a.connected) ??
    st.accounts.find((a) => a.connected);
  if (!acct) {
    throw new Error("No Google account connected — add one in Settings → Google Calendar.");
  }
  const cal =
    acct.calendars.find((c) => c.primary) ??
    acct.calendars.find((c) => c.access === "owner" || c.access === "writer");
  if (!cal) throw new Error(`No writable calendar found on ${acct.email}.`);
  await api.gcalCreateEvent({
    account: acct.email,
    calendarId: cal.id,
    title: p.title,
    date: p.date,
    start: p.start ?? undefined,
    end: p.end ?? undefined,
    addMeet: p.meet || undefined,
    guests: p.guests.length ? p.guests : undefined,
  });
  const when = p.start ? `${p.date} at ${p.start}` : p.date;
  return `Created “${p.title}” on ${when}${p.guests.length ? ` — invites sent to ${p.guests.join(", ")}` : ""}.`;
}
