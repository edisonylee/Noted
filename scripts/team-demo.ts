// An ephemeral, synthetic workspace for exercising organizational workflows.
// No local vault, calendar, Keychain item or real meeting is read.
import { TeamStore } from "../services/team/store";
import { createHandler } from "../services/team/server";
import { randomBytes } from "node:crypto";
const setupKey = randomBytes(32).toString("base64url");
const store = new TeamStore(":memory:", setupKey);
const setup = store.bootstrap(setupKey, "Fieldwork", "Alex Morgan");
const owner = store.authenticate(setup.token), org = setup.org, mainSpace = store.spaces(owner, org)[0].id;
const invite = store.invite(owner, org, { name: "Taylor Chen", role: "member" });
const member = store.accept(invite.token);
const taylor = store.authenticate(member.token!);
const product = store.saveFolder(owner, org, { space_id: mainSpace, name: "Product", description: "Research, priorities, and the decisions behind our work." });
const customers = store.saveFolder(owner, org, { space_id: mainSpace, name: "Customer conversations", description: "What we are learning from the people using Fieldwork." });
const restricted = store.createSpace(owner, org, { name: "Leadership", description: "Planning shared with leadership only.", visibility: "restricted" });
const research = store.saveGroup(owner, org, { name: "Product team", member_ids: [taylor] });
store.grant(owner, org, mainSpace, { kind: "group", id: research.id, role: "editor" });
const examples = [
  ["Launch readiness review", "2026-09-04", "## Decisions\n- Move the pilot launch to Friday. The export flow needs another review. [00:42]\n- Keep the first cohort to twelve teams.\n\n## Next steps\n- Taylor will finish the migration checklist by Thursday.\n- Alex will confirm the pilot roster with customer success.", product.id],
  ["Research playback: finding the right context", "2026-09-03", "## What we heard\nResearchers lose time reconstructing why a decision changed. A summary alone does not provide enough confidence.\n\n## Product direction\nShow the original conversation beside each important decision. Keep timestamps visible and make the source easy to open.", product.id],
  ["Northstar onboarding conversation", "2026-09-02", "## Customer context\nNorthstar has eight people sharing research across three time zones. They want project folders and an easy way to catch up on missed conversations.\n\n## Follow-up\nTaylor will share the onboarding checklist and schedule a pilot check-in.", customers.id],
  ["Weekly product priorities", "2026-09-01", "## Priorities\n1. Finish shared workspace permissions.\n2. Make transcript search useful for everyday questions.\n3. Put migration polish ahead of new integrations.\n\n## Open question\nHow should a team handle access after someone leaves?", product.id],
  ["Customer success handoff", "2026-08-31", "## Agreements\nEvery pilot team gets an owner and a shared folder. Success will be measured by whether people reuse meeting context during their first week.\n\n## Actions\nAlex owns the first-week check-in. Taylor owns the knowledge-base walkthrough.", customers.id],
];
examples.forEach(([title, date, summary, folder], i) => store.publish(i % 2 ? taylor : owner, org, { space_id: mainSpace, source_key: `sample-${i}`, title, summary, transcript: `[00:12] Alex: Let’s review ${title.toLowerCase()}.\n[00:42] Taylor: We should keep the source conversation available so the team can verify the decision.`, occurred_at: `${date}T16:00:00Z`, folder_ids: [folder] }));
store.publish(owner, org, { space_id: restricted.id, source_key: "sample-restricted", title: "Leadership planning", summary: "## Planning\nReview the next quarter’s hiring plan and runway assumptions with the leadership group.", occurred_at: "2026-09-04T09:00:00Z", folder_ids: [] });
store.saveRecipe(owner, org, { name: "What changed this week?", prompt: "Summarize decisions that changed this week. Cite the meetings and distinguish confirmed decisions from open questions.", kind: "recipe" });
store.saveRecipe(owner, org, { name: "Find open commitments", prompt: "List open commitments, their owners, and any stated deadlines. Cite the original conversation. Do not invent dates.", kind: "recipe" });
store.saveRecipe(owner, org, { name: "Customer interview", prompt: "Summarize customer goals, current workflow, friction, exact quotes, and follow-ups. Keep hypotheses separate from customer statements.", kind: "template" });
const handler = createHandler(store, ["http://127.0.0.1:1422", "http://localhost:1422"]);
const server = Bun.serve({ hostname: "127.0.0.1", port: 8790, maxRequestBodySize: 1_500_000, fetch: (req, s) => handler(req, s.requestIP(req)?.address) });
const vite = Bun.spawn(["node_modules/.bin/vite", "--config", "vite.team.config.ts"], { stdout: "inherit", stderr: "inherit" });
console.log("Preview: http://127.0.0.1:1422/team-preview.html");
console.log("Server: http://127.0.0.1:8790; choose Sign in with an access key");
console.log(`Owner sample access key: ${setup.token}`);
console.log(`Member sample access key: ${member.token}`);
const stop = () => { vite.kill(); server.stop(true); store.db.close(); process.exit(0); };
process.on("SIGINT", stop); process.on("SIGTERM", stop);
