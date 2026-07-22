import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

async function alphaBundleText() {
  const assets = await readdir(new URL("../dist/assets/", import.meta.url));
  const scripts = assets.filter((name) => name.endsWith(".js"));
  return Promise.all(
    scripts.map((name) => readFile(new URL(`../dist/assets/${name}`, import.meta.url), "utf8")),
  ).then((parts) => parts.join("\n"));
}

test("alpha bundle exposes the intended product", async () => {
  const bundle = await alphaBundleText();

  for (const required of ["Start recording", "My API keys", "Themes", "Google Calendar"]) {
    assert.ok(bundle.includes(required), `missing alpha capability: ${required}`);
  }

  for (const deferred of [
    "Noted Hosted",
    "Capture from your phone",
    "Detect speakers",
    "Record the meeting window as video",
    "Hosted Parakeet",
  ]) {
    assert.ok(!bundle.includes(deferred), `deferred capability leaked into alpha UI: ${deferred}`);
  }
});
