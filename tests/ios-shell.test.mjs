import assert from "node:assert/strict";
import { readdir, readFile, stat } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function read(relativePath) {
  return readFile(new URL(relativePath, root), "utf8");
}

test("iOS backend exports only local notes and health commands", async () => {
  const entry = await read("src-tauri/src/lib.rs");
  const mobile = await read("src-tauri/src/mobile.rs");

  assert.match(entry, /cfg\(not\(target_os = "ios"\)\)\]\s*include!\("desktop\.rs"\)/);
  assert.match(entry, /cfg\(target_os = "ios"\)\]\s*mod mobile/);
  for (const command of [
    "mobile_health",
    "list_mobile_notes",
    "create_mobile_note",
    "update_mobile_note",
    "delete_mobile_note",
    "export_mobile_notes",
    "restore_mobile_notes_export",
  ]) {
    assert.match(mobile, new RegExp(`\\b${command}\\b`));
  }

  for (const forbidden of [
    "meeting_start",
    "phone_info",
    "get_provider_settings",
    "agent_context_pending",
    "chat",
  ]) {
    assert.equal(mobile.includes(forbidden), false, `${forbidden} leaked into the iOS registry`);
  }
});

test("mobile notes use an isolated on-device SQLite store", async () => {
  const entry = await read("src-tauri/src/mobile.rs");
  const store = await read("src-tauri/src/mobile_store.rs");

  assert.match(entry, /noted-mobile\.sqlite3/);
  assert.match(store, /CREATE TABLE IF NOT EXISTS mobile_notes/);
  assert.match(store, /PRAGMA journal_mode = WAL/);
  assert.equal(store.includes("sqlite_vec"), false);
});

test("mobile note commands expose stable record IDs instead of SQLite row IDs", async () => {
  const entry = await read("src-tauri/src/mobile.rs");
  const store = await read("src-tauri/src/mobile_store.rs");
  const shell = await read("src/MobileShell.tsx");

  assert.match(entry, /record_id: String/);
  assert.doesNotMatch(entry, /\bid: i64\b/);
  assert.match(store, /pub record_id: String/);
  assert.match(store, /serde\(rename_all = "camelCase"\)/);
  assert.match(shell, /recordId: string/);
  assert.match(shell, /\{ recordId: draft\.recordId \}/);
  assert.doesNotMatch(shell, /\bid: number\b/);
});

test("desktop native dependencies are target-gated away from iOS", async () => {
  const manifest = await read("src-tauri/Cargo.toml");
  const desktopTable = manifest.indexOf("[target.'cfg(not(target_os = \"ios\"))'.dependencies]");
  assert.notEqual(desktopTable, -1);

  for (const dependency of ["sherpa-rs", "whisper-rs", "cpal", "tiny_http", "sqlite-vec"]) {
    assert.ok(manifest.indexOf(dependency) > desktopTable, `${dependency} must remain desktop-only`);
  }
});

test("generated iOS app requests no desktop recorder permissions", async () => {
  const info = await read("src-tauri/gen/apple/tauri-app_iOS/Info.plist");
  assert.equal(info.includes("NSMicrophoneUsageDescription"), false);
  assert.equal(info.includes("NSAudioCaptureUsageDescription"), false);
});

test("iOS signing configuration is reproducible", async () => {
  const config = JSON.parse(await read("src-tauri/tauri.ios.conf.json"));

  assert.equal(config.identifier, "com.noted.iphone");
  assert.equal(config.bundle.iOS.developmentTeam, "MYGAYC672C");
});

test("mobile frontend bundle excludes desktop command surfaces", async () => {
  const index = await read("dist-ios/index.html");
  assert.match(index, /assets\/index-[^\"]+\.js/);

  const assetsUrl = new URL("dist-ios/assets/", root);
  const assets = await readdir(assetsUrl);
  const scripts = assets.filter((name) => name.endsWith(".js"));
  assert.equal(scripts.length, 1, "the iPhone app should emit one isolated JavaScript entry");

  const scriptUrl = new URL(scripts[0], assetsUrl);
  const script = await readFile(scriptUrl, "utf8");
  const scriptSize = (await stat(scriptUrl)).size;
  assert.ok(scriptSize < 300_000, `mobile entry unexpectedly grew to ${scriptSize} bytes`);

  for (const forbidden of [
    "meeting_start",
    "phone_info",
    "get_provider_settings",
    "agent_context_pending",
    "Ollama",
    "Your companion is taking shape",
  ]) {
    assert.equal(script.includes(forbidden), false, `${forbidden} leaked into the mobile assets`);
  }
});
