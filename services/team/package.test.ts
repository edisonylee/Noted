import { test, expect } from "bun:test";
import { copyFileSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("the Docker runtime files start independently and expose message notification metadata", () => {
  const staging = mkdtempSync(join(tmpdir(), "noted-team-package-"));
  try {
    const dockerfile = readFileSync(join(import.meta.dir, "Dockerfile"), "utf8");
    const files = /^COPY (.+) \.\/$/m.exec(dockerfile)![1].split(/\s+/);
    const allowed = readFileSync(join(import.meta.dir, ".dockerignore"), "utf8").split("\n");
    for (const file of files) {
      expect(allowed).toContain(`!${file}`);
      copyFileSync(join(import.meta.dir, file), join(staging, file));
    }
    const result = Bun.spawnSync([process.execPath, "--eval", `
      import { openServiceStore, createHandler } from "./server.ts";
      const key = "package-test-setup-key-at-least-32-characters";
      const store = openServiceStore(":memory:", key);
      try {
        const session = store.bootstrap(key, "Package test", "Owner");
        const response = await createHandler(store)(new Request(
          "https://test.invalid/v1/orgs/" + session.org + "/chat-rooms",
          { headers: { Authorization: "Bearer " + session.token } },
        ));
        if (response.status !== 200) throw new Error("Room request failed");
        const [room] = await response.json();
        for (const field of ["notification_cursor", "latest_unread_message_seq", "notification_mode"])
          if (!(field in room)) throw new Error("Missing " + field);
      } finally { store.db.close(); }
    `], { cwd: staging, stdout: "pipe", stderr: "pipe" });
    expect(result.stderr.toString()).toBe("");
    expect(result.exitCode).toBe(0);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
});
