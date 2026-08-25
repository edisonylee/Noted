import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function read(relativePath) {
  return readFile(new URL(relativePath, root), "utf8");
}

test("fixture identity software keys remain gated to an opt-in DEBUG simulator", async () => {
  const manifest = await read("src-tauri/Cargo.toml");
  const runtime = await read("src-tauri/src/mobile_pairing_runtime.rs");
  const policy = await read(
    "src-tauri/crates/noted-apple-security/ios/Sources/NotedAppleSecurityCore/FixturePolicy.swift",
  );

  assert.match(manifest, /default\s*=\s*\[\]/);
  assert.match(
    manifest,
    /sanitized-development-fixtures\s*=\s*\[[\s\S]*noted-apple-security\/sanitized-development-fixtures/,
  );
  assert.match(runtime, /#\[cfg\(target_abi = "sim"\)\]/);
  assert.match(runtime, /prepare_sanitized_development_fixture_identity/);
  assert.match(runtime, /#\[cfg\(not\(target_abi = "sim"\)\)\][\s\S]*prepare_identity/);
  assert.match(policy, /isDebug\s*&&\s*isSimulator\s*&&\s*gate == exactGate/);
});

test("mobile commands expose pairing actions but never raw native crypto", async () => {
  const mobile = await read("src-tauri/src/mobile.rs");
  const handler = mobile.slice(mobile.indexOf("tauri::generate_handler!["));

  for (const command of [
    "mobile_pairing_status_fixture",
    "mobile_pairing_begin_fixture",
    "mobile_pairing_accept_server_hello_fixture",
    "mobile_pairing_confirm_fixture",
    "mobile_pairing_accept_bootstrap_fixture",
    "mobile_pairing_accept_server_finish_fixture",
    "mobile_pairing_discard_fixture",
  ]) {
    assert.ok(handler.includes(command), `${command} must be registered`);
  }
  for (const nativeMethod of [
    "verify_p256_signature",
    "sign_device",
    "open_authenticated_hpke",
    "stage_bootstrap_authenticated",
    "activate_bootstrap",
    "fresh_bytes",
    "fresh_uuid_v7",
  ]) {
    assert.equal(handler.includes(nativeMethod), false);
  }
});

test("desktop pairing uses the protocol lifetime and renews expired invitations", async () => {
  const authority = await read("src-tauri/src/fixture_authority_app.rs");
  const panel = await read("src/PhonePanel.tsx");

  assert.match(authority, /now \+ MAX_INVITATION_LIFETIME_MS/);
  assert.match(
    authority,
    /filter\(\|authority\| authority\.info\.invitation_expires_at_ms > now\)/,
  );
  assert.match(authority, /\*active = None;/);
  assert.doesNotMatch(authority, /10 \* 60 \* 1_000/);
  assert.match(panel, /const refresh = \(\) => api\.mobileAuthorityStart\(\)/);
  assert.match(panel, /window\.setInterval\(\(\) => \{[\s\S]*void refresh\(\)/);
});
