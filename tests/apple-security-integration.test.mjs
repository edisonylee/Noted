import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function read(relativePath) {
  return readFile(new URL(relativePath, root), "utf8");
}

test("iOS registers the native security boundary before mobile setup", async () => {
  const mobile = await read("src-tauri/src/mobile.rs");
  const manifest = await read("src-tauri/Cargo.toml");

  assert.match(
    manifest,
    /noted-apple-security\s*=\s*\{\s*path\s*=\s*"crates\/noted-apple-security"\s*\}/,
  );
  const securityPlugin = mobile.indexOf(".plugin(noted_apple_security::init())");
  const setup = mobile.indexOf(".setup(|app|");
  assert.ok(securityPlugin >= 0 && securityPlugin < setup);
  assert.equal(mobile.includes("prepare_sanitized_development_fixture_identity"), false);
  assert.equal(mobile.includes("prepare_identity("), false);
});

test("protected mobile storage stays closed until native hardening succeeds", async () => {
  const mobile = await read("src-tauri/src/mobile.rs");

  assert.match(mobile, /ProtectedMobileStore::closed/);
  assert.equal(mobile.includes("MobileStore::open("), false);
  assert.match(mobile, /prepare_store_directory/);
  assert.match(mobile, /protected_data_became_available/);
  assert.match(mobile, /harden_store_files/);
  assert.match(mobile, /report\.is_compliant\(\)/);
  assert.match(mobile, /identity_inventory\(\)/);
  assert.match(mobile, /inventory\.active\.len\(\) > 1/);
  assert.match(mobile, /inventory\.pending\.len\(\) > 1/);
  assert.match(mobile, /expected_device_id/);
  assert.match(mobile, /native_identity_reconciliation\(&self\.store\)/);
  assert.match(mobile, /"local"\s*=>\s*Ok\(NativeIdentityRequirement::FreshUnpaired\)/);
  assert.match(
    mobile,
    /inventory\.active\.len\(\) != 1 \|\| !inventory\.pending\.is_empty\(\)/,
  );
  assert.match(
    mobile,
    /paired mobile replica requires exactly one matching active native identity/,
  );
  assert.match(mobile, /native identity is not bound to the current mobile replica/);
  assert.match(mobile, /multiple live native identities require explicit recovery/);

  const preHarden = mobile.indexOf(
    ".harden_store_files(database_path, &preexisting_recovery_paths)",
  );
  const open = mobile.indexOf("self.store.protected_data_became_available()?");
  const harden = mobile.indexOf(".harden_store_files(database_path, &recovery_paths)");
  const publish = mobile.indexOf("lifecycle.ready = true");
  assert.ok(
    preHarden >= 0 && preHarden < open && open < harden && harden < publish,
  );
  assert.match(mobile, /existing_migration_recovery_paths/);
  assert.match(mobile, /require_hardened_before_open/);
  assert.match(mobile, /validate_preexisting_sqlite_paths\(database_path\)/);
  assert.match(mobile, /symlink_metadata\(path\)/);
  assert.match(mobile, /orphaned mobile SQLite sidecar exists without its database/);
  assert.match(
    mobile,
    /commit_completed_native_discard\(&self\.store, &native_inventory\)[\s\S]*?native_identity_reconciliation\(&self\.store\)/,
  );
  assert.match(
    mobile,
    /checkpoint_after_completed_discard\(&checkpoint, &snapshots, updated_at\)/,
  );
  assert.match(
    mobile,
    /signing_public_key: identity\.signing_public_key\.clone\(\)[\s\S]*?hpke_public_key: identity\.hpke_public_key\.clone\(\)/,
  );

  assert.match(mobile, /if let Err\(error\) = result/);
  assert.match(mobile, /lifecycle\.ready = false/);
  assert.match(mobile, /failed to close the mobile store after the security check/);
});

test("lock notifications and foreground reconciliation fail closed", async () => {
  const mobile = await read("src-tauri/src/mobile.rs");

  assert.match(mobile, /struct ProtectedDataGate[\s\S]*?available:\s*AtomicBool/);
  assert.match(mobile, /unavailable_epoch:\s*AtomicU64/);
  assert.match(
    mobile,
    /protected_data_became_unavailable[\s\S]*?begin_unavailable\(\)[\s\S]*?lifecycle\s*=\s*self[\s\S]*?\.lifecycle[\s\S]*?\.lock\(\)/,
  );
  assert.match(
    mobile,
    /with_ready_store[\s\S]*?is_available\(\)[\s\S]*?lifecycle[\s\S]*?\.lock\(\)[\s\S]*?is_available\(\)/,
  );
  assert.match(mobile, /publish_if_epoch_unchanged/);
  assert.match(mobile, /if self\.epoch\(\) != expected_epoch/);
  assert.match(mobile, /subscribe_protected_data/);
  assert.match(
    mobile,
    /ProtectedDataState::Unavailable\s*=>[\s\S]*?protected_data_became_unavailable\(\)/,
  );
  assert.match(
    mobile,
    /ProtectedDataState::Available\s*=>[\s\S]*?schedule_available_reconciliation/,
  );
  assert.match(mobile, /tauri::RunEvent::Resumed/);
  assert.match(mobile, /schedule_available_reconciliation\(app_handle\.clone\(\), true\)/);
  assert.match(mobile, /with_ready_store/);
  assert.match(mobile, /MOBILE_STORE_LOCKED_ERROR/);
});

test("native security methods are not exposed as JavaScript commands", async () => {
  const mobile = await read("src-tauri/src/mobile.rs");
  const handler = mobile.slice(mobile.indexOf("tauri::generate_handler!["));

  for (const nativeMethod of [
    "prepare_identity",
    "identity_inventory",
    "open_authenticated_hpke",
    "stage_bootstrap_authenticated",
    "activate_bootstrap",
    "discard_pending",
    "protected_data_state",
    "harden_store_files",
  ]) {
    assert.equal(
      handler.includes(nativeMethod),
      false,
      `${nativeMethod} must remain behind the Rust/native boundary`,
    );
  }
});
