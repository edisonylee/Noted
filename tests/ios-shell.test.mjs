import assert from "node:assert/strict";
import { readdir, readFile, stat } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function read(relativePath) {
  return readFile(new URL(relativePath, root), "utf8");
}

test("iOS backend exports only the feasibility health command", async () => {
  const entry = await read("src-tauri/src/lib.rs");
  const mobile = await read("src-tauri/src/mobile.rs");

  assert.match(entry, /cfg\(not\(target_os = "ios"\)\)\]\s*include!\("desktop\.rs"\)/);
  assert.match(entry, /cfg\(target_os = "ios"\)\]\s*mod mobile/);
  assert.match(mobile, /generate_handler!\[mobile_health\]/);

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
  assert.equal(scripts.length, 1, "the feasibility shell should emit one JavaScript entry");

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
  ]) {
    assert.equal(script.includes(forbidden), false, `${forbidden} leaked into the mobile assets`);
  }
});
