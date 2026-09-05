import type { TeamStore } from "./store";

// These fictional conversations live in their own organization, never the vault
// or a real team's workspace. Re-running local setup does not duplicate them.
export function ensureExampleWorkspace(store: TeamStore, owner: string) {
  const existing = store
    .orgs(owner)
    .find((org) => org.name === "Noted examples");
  if (existing) return String(existing.id);
  const org = store.createOrg(owner, "Noted examples");
  const space = store.spaces(owner, org)[0];
  store.updateSpace(owner, org, space.id, {
    name: "Example meetings",
    description:
      "Fictional conversations for trying search, sources, and follow-up chat. Your real meetings stay in your private library.",
    visibility: "team",
  });
  const folder = store.saveFolder(owner, org, {
    space_id: space.id,
    name: "Pilot project",
    description: "A sample project with decisions, owners, and follow-ups.",
  });
  const examples = [
    {
      title: "Example — Pilot launch review",
      summary:
        "## Decision\nThe pilot launches Friday with twelve teams.\n\n## Next steps\n- Taylor owns the migration checklist, due Thursday. [00:42]\n- Alex will confirm the pilot roster with customer success.\n\nThis is a fictional example meeting.",
      transcript:
        "[00:12] Alex: Let's launch the pilot on Friday with twelve teams.\n[00:42] Taylor: I'll finish the migration checklist by Thursday.\n[01:05] Alex: I'll confirm the pilot roster with customer success.",
    },
    {
      title: "Example — Customer research playback",
      summary:
        "## Research findings\nParticipants want project folders and evidence beside each important decision.\n\n## Follow-up\nTaylor will test transcript links with the pilot group next week.\n\nThis is a fictional example meeting.",
      transcript:
        "[00:10] Alex: Participants keep asking why a decision changed.\n[00:30] Taylor: They want the original conversation beside the summary. I'll test transcript links with the pilot group next week.",
    },
    {
      title: "Example — Customer success handoff",
      summary:
        "## Agreements\nEach pilot team gets an owner and a project folder.\n\n## Owners\nAlex owns the first-week check-in. Taylor owns the knowledge-base walkthrough.\n\nThis is a fictional example meeting.",
      transcript:
        "[00:15] Alex: I'll own the first-week check-in for the pilot teams.\n[00:28] Taylor: I'll take the knowledge-base walkthrough. Each team should have a project folder.",
    },
  ];
  examples.forEach((example, index) =>
    store.publish(owner, org, {
      ...example,
      space_id: space.id,
      folder_ids: [folder.id],
      source_key: `noted-example-${index}`,
      occurred_at: new Date(Date.now() - index * 86400_000).toISOString(),
    }),
  );
  store.saveRecipe(owner, org, {
    name: "Find open commitments",
    kind: "recipe",
    prompt:
      "List commitments, owners and any stated deadlines. Cite each meeting. Do not invent dates or owners.",
  });
  store.saveRecipe(owner, org, {
    name: "Draft a follow-up",
    kind: "recipe",
    prompt:
      "Draft a concise follow-up with decisions, action owners and stated deadlines. Cite your sources. Do not send anything.",
  });
  return org;
}
