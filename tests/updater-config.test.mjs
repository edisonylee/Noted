import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const read = (path) => readFile(new URL(path, root), "utf8");

test("public beta emits signed updater artifacts", async () => {
  const config = JSON.parse(await read("src-tauri/tauri.beta.conf.json"));
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.match(config.plugins.updater.pubkey, /^[A-Za-z0-9+/=]+$/);
  assert.deepEqual(config.plugins.updater.endpoints, [
    "https://github.com/edisonylee/Noted/releases/latest/download/latest.json",
  ]);
});

test("updater is release-gated and wired through Tauri permissions", async () => {
  const [source, desktop, capability, workflow] = await Promise.all([
    read("src/appUpdater.ts"),
    read("src-tauri/src/desktop.rs"),
    read("src-tauri/capabilities/default.json"),
    read(".github/workflows/macos-beta.yml"),
  ]);
  assert.match(source, /VITE_NOTED_UPDATES === "1"/);
  assert.match(desktop, /tauri_plugin_updater::Builder::new\(\)\.build\(\)/);
  assert.match(desktop, /tauri_plugin_process::init\(\)/);
  assert.match(capability, /"updater:default"/);
  assert.match(capability, /"process:allow-restart"/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY/);
  assert.match(workflow, /VITE_NOTED_UPDATES: "1"/);
});
