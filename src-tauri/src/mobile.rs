use crate::direct_sync::DirectSyncLimits;
use crate::direct_sync_transport::{
    PairingEndpoint, PrivateLanDirectSyncSession, PrivateLanEndpointCandidate,
    PrivateLanPairingSession,
};
use crate::mobile_deep_link::MobileDeepLink;
use crate::mobile_notes_sync::{MobileNotesSyncOrchestrator, MobileNotesSyncReport};
use crate::mobile_pairing_runtime::{
    accept_bootstrap, accept_bootstrap_poll_response, accept_server_finish, accept_server_hello,
    begin_fixture_pairing, bootstrap_metadata_from_apple, checkpoint_after_completed_discard,
    confirm_fixture_pairing, create_bootstrap_poll, discard_fixture_pairing,
    recover_fixture_pairing, FixturePairingStatus, NativeBootstrapSnapshot, NativeIdentitySnapshot,
    NativePairingLifecycle, NativeSigningKeyBacking,
};
use crate::mobile_store::{
    MobileNote, MobileNotesWorkspace, MobileStore, MobileStoreHealth, MobileWorkspaceNote,
    MOBILE_STORE_LOCKED_ERROR,
};
use crate::mobile_sync_native::AppleMobileSyncCrypto;
use crate::mobile_sync_runtime::ExactRequestJournal;
use crate::mobile_sync_store_adapter::MobileStoreExactRequestJournal;
use crate::pairing_client::PairingClientState;
use crate::pairing_protocol::{Invitation, TransportEvidence};
use noted_apple_security::{
    AppleSecurity, AppleSecurityExt, IdentityHandle, IdentityInventory, IdentityLifecycle,
    ProtectedDataEvent, ProtectedDataState, SigningKeyBacking, StoreProtectionReport,
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Runtime, State, Wry};

#[derive(Debug, Default)]
struct ProtectedDataGate {
    available: AtomicBool,
    unavailable_epoch: AtomicU64,
}

impl ProtectedDataGate {
    fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }

    fn epoch(&self) -> u64 {
        self.unavailable_epoch.load(Ordering::Acquire)
    }

    /// Records an unavailable notification before its callback waits for the
    /// lifecycle mutex. A stale reconciliation can therefore never hide it.
    fn begin_unavailable(&self) -> u64 {
        let epoch = self
            .unavailable_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_or(u64::MAX, |previous| previous + 1);
        self.available.store(false, Ordering::Release);
        epoch
    }

    fn force_closed(&self) {
        self.available.store(false, Ordering::Release);
    }

    /// Called only while the lifecycle mutex is held. Commands also take that
    /// mutex, so the post-store epoch check closes the only publication gap.
    fn publish_if_epoch_unchanged(&self, expected_epoch: u64) -> bool {
        if expected_epoch == u64::MAX || self.epoch() != expected_epoch {
            return false;
        }
        self.available.store(true, Ordering::Release);
        if self.epoch() != expected_epoch {
            self.available.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

struct ProtectedMobileStore {
    store: MobileStore,
    lifecycle: Mutex<ProtectedMobileLifecycle>,
    protected_data: ProtectedDataGate,
    direct_sync: tokio::sync::Mutex<()>,
}

struct ProtectedMobileLifecycle {
    ready: bool,
    closed_unavailable_epoch: u64,
    // Public handles only. Keeping the complete inventory lets a future pairing
    // coordinator reconcile a crash without guessing which Keychain item won.
    identity_inventory: Option<IdentityInventory>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeIdentityRequirement {
    FreshUnpaired,
    PairingPending,
    ActivationTransition,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedNativeIdentity {
    handle: String,
    signing_public_key: Vec<u8>,
    hpke_public_key: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeIdentityReconciliation {
    requirement: NativeIdentityRequirement,
    expected_identity: Option<ExpectedNativeIdentity>,
}

impl ProtectedMobileStore {
    fn closed(path: &Path) -> Self {
        Self {
            store: MobileStore::closed(path),
            lifecycle: Mutex::new(ProtectedMobileLifecycle {
                ready: false,
                closed_unavailable_epoch: 0,
                identity_inventory: None,
            }),
            protected_data: ProtectedDataGate::default(),
            direct_sync: tokio::sync::Mutex::new(()),
        }
    }

    fn with_ready_store<T>(
        &self,
        operation: impl FnOnce(&MobileStore) -> Result<T, String>,
    ) -> Result<T, String> {
        if !self.protected_data.is_available() {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }
        let lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "mobile protected-data lifecycle lock was poisoned".to_string())?;
        if !self.protected_data.is_available() || !lifecycle.ready {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }
        operation(&self.store)
    }

    /// The native unavailable callback calls this directly, rather than
    /// scheduling work, so returning from the callback means SQLite is closed.
    fn protected_data_became_unavailable(&self) -> Result<(), String> {
        // Increment before waiting behind an in-flight DB operation. A stale
        // opener cannot overwrite this event with an `available = true` store.
        let event_epoch = self.protected_data.begin_unavailable();
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "mobile protected-data lifecycle lock was poisoned".to_string())?;
        if lifecycle.closed_unavailable_epoch >= event_epoch {
            return Ok(());
        }
        self.close_for_unavailable(&mut lifecycle)
    }

    fn reconcile_protected_data<R: Runtime>(
        &self,
        security: &AppleSecurity<R>,
        force_reopen: bool,
    ) -> Result<(), String> {
        if force_reopen {
            self.protected_data.force_closed();
        }
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "mobile protected-data lifecycle lock was poisoned".to_string())?;

        // An unavailable callback may have published its epoch before this
        // reconciliation won the mutex. Close/acknowledge that event first;
        // the delayed callback then becomes an idempotent no-op.
        if lifecycle.closed_unavailable_epoch != self.protected_data.epoch() {
            self.close_for_unavailable(&mut lifecycle)?;
        }

        let protected_state = security
            .protected_data_state()
            .map_err(|error| error.to_string())?;
        if protected_state == ProtectedDataState::Unavailable {
            self.protected_data.begin_unavailable();
            return self.close_for_unavailable(&mut lifecycle);
        }
        let reconciliation_epoch = self.protected_data.epoch();

        if lifecycle.ready && !force_reopen {
            let result = self.verify_open_store(security, &mut lifecycle, reconciliation_epoch);
            if let Err(error) = result {
                self.protected_data.force_closed();
                return match self.close_for_unavailable(&mut lifecycle) {
                    Ok(()) => Err(error),
                    Err(close_error) => Err(format!(
                        "{error}; failed to close the mobile store after the security check: {close_error}"
                    )),
                };
            }
            return Ok(());
        }

        // Commands take the same lifecycle mutex, so they cannot observe the
        // reopened connection until post-open hardening and inventory
        // reconciliation have both succeeded.
        lifecycle.ready = false;
        lifecycle.identity_inventory = None;
        self.store.protected_data_became_unavailable()?;

        let result = self.open_hardened_store(security, &mut lifecycle, reconciliation_epoch);
        if let Err(error) = result {
            self.protected_data.force_closed();
            return match self.close_for_unavailable(&mut lifecycle) {
                Ok(()) => Err(error),
                Err(close_error) => Err(format!(
                    "{error}; failed to close the mobile store after the security check: {close_error}"
                )),
            };
        }
        Ok(())
    }

    fn close_for_unavailable(
        &self,
        lifecycle: &mut ProtectedMobileLifecycle,
    ) -> Result<(), String> {
        self.protected_data.force_closed();
        lifecycle.ready = false;
        lifecycle.identity_inventory = None;
        self.store.protected_data_became_unavailable()?;
        lifecycle.closed_unavailable_epoch = self.protected_data.epoch();
        Ok(())
    }

    fn open_hardened_store<R: Runtime>(
        &self,
        security: &AppleSecurity<R>,
        lifecycle: &mut ProtectedMobileLifecycle,
        reconciliation_epoch: u64,
    ) -> Result<(), String> {
        let database_path = self.store.path();
        let directory = database_path
            .parent()
            .ok_or_else(|| "mobile database path has no parent directory".to_string())?;
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let preexisting_recovery_paths = existing_migration_recovery_paths(database_path)?;

        let prepared = security
            .prepare_store_directory(database_path, &preexisting_recovery_paths)
            .map_err(|error| error.to_string())?;
        require_prepared_directory(&prepared)?;

        let database_exists = validate_preexisting_sqlite_paths(database_path)?;
        if database_exists {
            require_hardened_before_open(
                security
                    .harden_store_files(database_path, &preexisting_recovery_paths)
                    .map_err(|error| error.to_string())?,
            )?;
        } else if !preexisting_recovery_paths.is_empty() {
            return Err("mobile migration recovery files exist without their database".to_string());
        }
        if security
            .protected_data_state()
            .map_err(|error| error.to_string())?
            != ProtectedDataState::Available
        {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }

        // MobileStore forces WAL mode as part of this reopen. The outer
        // lifecycle lock keeps every command blocked until the DB, WAL, SHM,
        // and any migration snapshot pass the native post-open check.
        self.store.protected_data_became_available()?;
        let recovery_paths = recovery_paths(&self.store)?;
        require_compliant_store(
            security
                .harden_store_files(database_path, &recovery_paths)
                .map_err(|error| error.to_string())?,
        )?;

        let expected_device_id = self.store.replica_device_id()?;
        let mut native_inventory = security
            .identity_inventory()
            .map_err(|error| error.to_string())?;
        commit_completed_native_discard(&self.store, &native_inventory)?;
        if commit_native_authority_revocation(&self.store, security)? {
            native_inventory = security
                .identity_inventory()
                .map_err(|error| error.to_string())?;
        }
        let identity_reconciliation = native_identity_reconciliation(&self.store)?;
        let inventory = reconcile_identity_inventory(
            native_inventory,
            &expected_device_id,
            identity_reconciliation.requirement,
            identity_reconciliation.expected_identity.as_ref(),
        )?;
        if security
            .protected_data_state()
            .map_err(|error| error.to_string())?
            != ProtectedDataState::Available
        {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }

        lifecycle.identity_inventory = Some(inventory);
        lifecycle.ready = true;
        if !self
            .protected_data
            .publish_if_epoch_unchanged(reconciliation_epoch)
        {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }
        Ok(())
    }

    fn verify_open_store<R: Runtime>(
        &self,
        security: &AppleSecurity<R>,
        lifecycle: &mut ProtectedMobileLifecycle,
        reconciliation_epoch: u64,
    ) -> Result<(), String> {
        let recovery_paths = recovery_paths(&self.store)?;
        require_compliant_store(
            security
                .harden_store_files(self.store.path(), &recovery_paths)
                .map_err(|error| error.to_string())?,
        )?;
        let expected_device_id = self.store.replica_device_id()?;
        let mut native_inventory = security
            .identity_inventory()
            .map_err(|error| error.to_string())?;
        commit_completed_native_discard(&self.store, &native_inventory)?;
        if commit_native_authority_revocation(&self.store, security)? {
            native_inventory = security
                .identity_inventory()
                .map_err(|error| error.to_string())?;
        }
        let identity_reconciliation = native_identity_reconciliation(&self.store)?;
        let inventory = reconcile_identity_inventory(
            native_inventory,
            &expected_device_id,
            identity_reconciliation.requirement,
            identity_reconciliation.expected_identity.as_ref(),
        )?;
        if security
            .protected_data_state()
            .map_err(|error| error.to_string())?
            != ProtectedDataState::Available
        {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }
        lifecycle.identity_inventory = Some(inventory);
        lifecycle.ready = true;
        if !self
            .protected_data
            .publish_if_epoch_unchanged(reconciliation_epoch)
        {
            return Err(MOBILE_STORE_LOCKED_ERROR.to_string());
        }
        Ok(())
    }
}

fn recovery_paths(store: &MobileStore) -> Result<Vec<PathBuf>, String> {
    let Some(stored_path) = store.migration_recovery_path()? else {
        return Ok(Vec::new());
    };
    Ok(vec![reanchor_migration_recovery_path(
        store.path(),
        Path::new(&stored_path),
    )?])
}

/// iOS may assign a new data-container UUID when a development build is
/// reinstalled while preserving the app's data. Migration rows created by an
/// earlier build therefore cannot be trusted as durable absolute paths. Keep
/// only the generated recovery filename and resolve it beneath the current
/// database directory before asking the native protection layer to inspect it.
fn reanchor_migration_recovery_path(
    database_path: &Path,
    stored_path: &Path,
) -> Result<PathBuf, String> {
    let parent = database_path
        .parent()
        .ok_or_else(|| "mobile database path has no parent directory".to_string())?;
    let file_name = stored_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "mobile migration recovery path has no valid filename".to_string())?;
    if !file_name.starts_with("noted-mobile-pre-schema-v") || !file_name.ends_with(".sqlite3") {
        return Err("mobile migration recovery filename is invalid".to_string());
    }
    Ok(parent.join("migration-recovery").join(file_name))
}

fn existing_migration_recovery_paths(database_path: &Path) -> Result<Vec<PathBuf>, String> {
    let parent = database_path
        .parent()
        .ok_or_else(|| "mobile database path has no parent directory".to_string())?;
    let recovery_directory = parent.join("migration-recovery");
    let entries = match fs::read_dir(&recovery_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err(
                "mobile migration recovery directory contains a non-regular entry".to_string(),
            );
        }
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

fn validate_preexisting_sqlite_paths(database_path: &Path) -> Result<bool, String> {
    let mut wal_path = database_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let mut shm_path = database_path.as_os_str().to_os_string();
    shm_path.push("-shm");
    let paths = [
        (database_path.to_path_buf(), "database"),
        (PathBuf::from(wal_path), "WAL sidecar"),
        (PathBuf::from(shm_path), "SHM sidecar"),
    ];
    let mut present = [false; 3];
    for (index, (path, label)) in paths.iter().enumerate() {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => present[index] = true,
            Ok(_) => return Err(format!("mobile SQLite {label} path is not a regular file")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    if !present[0] && (present[1] || present[2]) {
        return Err("orphaned mobile SQLite sidecar exists without its database".to_string());
    }
    Ok(present[0])
}

fn require_prepared_directory(report: &StoreProtectionReport) -> Result<(), String> {
    if report.protection_class == "NSFileProtectionComplete" && report.violations.is_empty() {
        Ok(())
    } else {
        Err("mobile store directory did not satisfy the native protection policy".to_string())
    }
}

fn require_hardened_before_open(report: StoreProtectionReport) -> Result<(), String> {
    // WAL/SHM may not exist until SQLite opens. Every path that does exist has
    // already been explicitly hardened and verified; post-open reconciliation
    // remains responsible for proving the newly created sidecars too.
    if report.protection_class == "NSFileProtectionComplete" && report.violations.is_empty() {
        Ok(())
    } else {
        Err(
            "existing mobile SQLite files did not satisfy the pre-open protection policy"
                .to_string(),
        )
    }
}

fn require_compliant_store(report: StoreProtectionReport) -> Result<(), String> {
    if report.is_compliant() {
        Ok(())
    } else {
        Err("mobile SQLite files did not satisfy the native protection policy".to_string())
    }
}

fn native_identity_reconciliation(
    store: &MobileStore,
) -> Result<NativeIdentityReconciliation, String> {
    let revoked = store.authority_revocation()?.is_some();
    if let Some(checkpoint) = store.load_pairing_checkpoint()? {
        if revoked && checkpoint.client.state == PairingClientState::Active {
            return Ok(NativeIdentityReconciliation {
                requirement: NativeIdentityRequirement::FreshUnpaired,
                expected_identity: None,
            });
        }
        let requirement = match checkpoint.client.state {
            PairingClientState::Cancelled => NativeIdentityRequirement::FreshUnpaired,
            PairingClientState::PendingActivation => {
                NativeIdentityRequirement::ActivationTransition
            }
            PairingClientState::Active => NativeIdentityRequirement::Active,
            _ => NativeIdentityRequirement::PairingPending,
        };
        let expected_identity =
            (checkpoint.client.state != PairingClientState::Cancelled).then(|| {
                ExpectedNativeIdentity {
                    handle: checkpoint.identity_handle,
                    signing_public_key: checkpoint.client.identity.signing_public_key,
                    hpke_public_key: checkpoint.client.identity.hpke_public_key,
                }
            });
        return Ok(NativeIdentityReconciliation {
            requirement,
            expected_identity,
        });
    }
    Ok(NativeIdentityReconciliation {
        requirement: native_identity_requirement_from_health(&store.health()?)?,
        expected_identity: None,
    })
}

fn commit_native_authority_revocation<R: Runtime>(
    store: &MobileStore,
    security: &AppleSecurity<R>,
) -> Result<bool, String> {
    let Some(revocation) = store.authority_revocation()? else {
        return Ok(false);
    };
    let Some(checkpoint) = store.load_pairing_checkpoint()? else {
        return Err("revoked enrollment is missing its durable pairing checkpoint".to_string());
    };
    // A later-generation pending checkpoint proves the old active identity was
    // already retired before re-enrollment began. Never target the new handle.
    if checkpoint.client.state != PairingClientState::Active {
        return Ok(false);
    }
    let identity = IdentityHandle::from_opaque(&checkpoint.identity_handle)
        .map_err(|error| error.to_string())?;
    let authority_generation = u64::try_from(revocation.authority_generation)
        .map_err(|_| "revocation authority generation exceeds u64".to_string())?;
    let purge_generation = u64::try_from(revocation.purge_generation)
        .map_err(|_| "revocation purge generation exceeds u64".to_string())?;
    let key_epoch = u64::try_from(revocation.key_epoch)
        .map_err(|_| "revocation key epoch exceeds u64".to_string())?;
    let retired = security
        .revoke_active(
            &identity,
            &revocation.receipt_id,
            authority_generation,
            purge_generation,
            key_epoch,
        )
        .map_err(|error| error.to_string())?;
    if retired.handle != identity
        || retired.device_id != revocation.device_id
        || retired.lifecycle != IdentityLifecycle::Discarded
    {
        return Err("native revocation did not retire the authenticated identity".to_string());
    }
    Ok(true)
}

fn commit_completed_native_discard(
    store: &MobileStore,
    inventory: &IdentityInventory,
) -> Result<bool, String> {
    let Some(checkpoint) = store.load_pairing_checkpoint()? else {
        return Ok(false);
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let updated_at =
        i64::try_from(now).map_err(|_| "system clock exceeds i64 milliseconds".to_string())?;
    let snapshots = pairing_inventory_snapshots(inventory)?;
    let Some(completed) = checkpoint_after_completed_discard(&checkpoint, &snapshots, updated_at)
    else {
        return Ok(false);
    };
    store.save_pairing_checkpoint(&completed)?;
    Ok(true)
}

fn pairing_inventory_snapshots(
    inventory: &IdentityInventory,
) -> Result<Vec<NativeIdentitySnapshot>, String> {
    inventory
        .pending
        .iter()
        .chain(inventory.active.iter())
        .chain(inventory.discarded.iter())
        .map(|identity| {
            Ok(NativeIdentitySnapshot {
                handle: identity.handle.expose_opaque().to_string(),
                device_id: identity.device_id.clone(),
                signing_public_key: identity.signing_public_key.clone(),
                hpke_public_key: identity.hpke_public_key.clone(),
                signing_key_backing: match identity.signing_key_backing {
                    SigningKeyBacking::SecureEnclave => NativeSigningKeyBacking::SecureEnclave,
                    SigningKeyBacking::SoftwareFixture => NativeSigningKeyBacking::SoftwareFixture,
                },
                lifecycle: match identity.lifecycle {
                    IdentityLifecycle::Pending => NativePairingLifecycle::Pending,
                    IdentityLifecycle::Active => NativePairingLifecycle::Active,
                    IdentityLifecycle::Discarded => NativePairingLifecycle::Discarded,
                },
                bootstrap: identity
                    .bootstrap_recovery
                    .as_ref()
                    .map(|binding| {
                        Ok::<NativeBootstrapSnapshot, String>(NativeBootstrapSnapshot {
                            handle: binding.pending_bootstrap_handle.expose_opaque().to_string(),
                            receipt_id: binding.receipt_id.clone(),
                            envelope_digest: binding.envelope_digest.clone(),
                            metadata: bootstrap_metadata_from_apple(&binding.metadata)?,
                        })
                    })
                    .transpose()?,
            })
        })
        .collect()
}

fn native_identity_requirement_from_health(
    health: &MobileStoreHealth,
) -> Result<NativeIdentityRequirement, String> {
    match health.sync.as_str() {
        // MobileStore::health emits `local` only while the durable replica is
        // explicitly `local_staging`. Every paired state, including a paired
        // replica whose enrollment is incomplete or revoked, fails closed.
        "local" => Ok(NativeIdentityRequirement::FreshUnpaired),
        "not_enrolled" | "pending" | "syncing" | "synced" | "error" => {
            Ok(NativeIdentityRequirement::Active)
        }
        state => Err(format!(
            "mobile replica reported unsupported native identity state {state}"
        )),
    }
}

fn reconcile_identity_inventory(
    inventory: IdentityInventory,
    expected_device_id: &str,
    requirement: NativeIdentityRequirement,
    expected_identity: Option<&ExpectedNativeIdentity>,
) -> Result<IdentityInventory, String> {
    if inventory.active.len() > 1
        || inventory.pending.len() > 1
        || (!inventory.active.is_empty() && !inventory.pending.is_empty())
    {
        return Err(
            "multiple live native identities require explicit recovery; none was selected"
                .to_string(),
        );
    }

    let mut handles = BTreeSet::new();
    for (identities, expected_lifecycle) in [
        (&inventory.pending, IdentityLifecycle::Pending),
        (&inventory.active, IdentityLifecycle::Active),
        (&inventory.discarded, IdentityLifecycle::Discarded),
    ] {
        for identity in identities {
            if identity.lifecycle != expected_lifecycle {
                return Err("native identity inventory lifecycle mismatch".to_string());
            }
            if !handles.insert(identity.handle.expose_opaque()) {
                return Err("native identity inventory contains duplicate identities".to_string());
            }
            if expected_lifecycle != IdentityLifecycle::Discarded
                && identity.device_id != expected_device_id
            {
                return Err(
                    "native identity is not bound to the current mobile replica; explicit recovery is required"
                        .to_string(),
                );
            }
        }
    }

    if requirement == NativeIdentityRequirement::Active
        && (inventory.active.len() != 1 || !inventory.pending.is_empty())
    {
        return Err(
            "paired mobile replica requires exactly one matching active native identity; explicit recovery is required"
                .to_string(),
        );
    }

    if requirement == NativeIdentityRequirement::PairingPending
        && (inventory.pending.len() != 1 || !inventory.active.is_empty())
    {
        return Err(
            "in-progress pairing requires exactly one matching pending native identity".to_string(),
        );
    }

    if requirement == NativeIdentityRequirement::ActivationTransition
        && !((inventory.pending.len() == 1 && inventory.active.is_empty())
            || (inventory.active.len() == 1 && inventory.pending.is_empty()))
    {
        return Err(
            "pairing activation recovery requires exactly one matching live native identity"
                .to_string(),
        );
    }

    // A single pending identity without a checkpoint is the explicit-discard
    // recovery case for a crash between native creation and the first SQLite
    // commit. An active identity can never be treated as fresh/unpaired.
    if requirement == NativeIdentityRequirement::FreshUnpaired && !inventory.active.is_empty() {
        return Err("unpaired mobile replica cannot select an active native identity".to_string());
    }

    if let Some(expected) = expected_identity {
        let live_identity = match requirement {
            NativeIdentityRequirement::PairingPending => inventory.pending.first(),
            NativeIdentityRequirement::ActivationTransition => inventory
                .pending
                .first()
                .or_else(|| inventory.active.first()),
            NativeIdentityRequirement::Active => inventory.active.first(),
            NativeIdentityRequirement::FreshUnpaired => None,
        }
        .ok_or_else(|| {
            "durable pairing checkpoint has no matching live native identity".to_string()
        })?;
        if live_identity.handle.expose_opaque() != expected.handle
            || live_identity.signing_public_key != expected.signing_public_key
            || live_identity.hpke_public_key != expected.hpke_public_key
        {
            return Err(
                "live native identity does not match the durable pairing checkpoint handle and public keys"
                    .to_string(),
            );
        }
    }

    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use noted_apple_security::{IdentityHandle, PublicIdentity, SigningKeyBacking};

    const DEVICE_ID: &str = "018f47f2-8ee8-7a28-91eb-9b3f2619e07e";
    const OTHER_DEVICE_ID: &str = "018f47f2-8ee8-7a28-91eb-9b3f2619e07f";

    fn identity(handle: &str, device_id: &str, lifecycle: IdentityLifecycle) -> PublicIdentity {
        PublicIdentity {
            handle: serde_json::from_str::<IdentityHandle>(&format!("\"{handle}\""))
                .expect("deserialize opaque test handle"),
            device_id: device_id.to_string(),
            signing_public_key: vec![4; 65],
            hpke_public_key: vec![7; 32],
            lifecycle,
            signing_key_backing: SigningKeyBacking::SecureEnclave,
            bootstrap_recovery: None,
        }
    }

    fn empty_inventory() -> IdentityInventory {
        IdentityInventory {
            pending: Vec::new(),
            active: Vec::new(),
            discarded: Vec::new(),
        }
    }

    #[test]
    fn migration_recovery_path_follows_the_current_ios_container() {
        let database = Path::new(
            "/private/var/mobile/Containers/Data/Application/NEW/Library/Application Support/com.noted.iphone/noted-mobile.sqlite3",
        );
        let stale = Path::new(
            "/private/var/mobile/Containers/Data/Application/OLD/Library/Application Support/com.noted.iphone/migration-recovery/noted-mobile-pre-schema-v8-1781-123-0.sqlite3",
        );

        assert_eq!(
            reanchor_migration_recovery_path(database, stale).expect("reanchor recovery path"),
            PathBuf::from(
                "/private/var/mobile/Containers/Data/Application/NEW/Library/Application Support/com.noted.iphone/migration-recovery/noted-mobile-pre-schema-v8-1781-123-0.sqlite3",
            )
        );
    }

    #[test]
    fn migration_recovery_path_rejects_unrecognized_filenames() {
        let database = Path::new("/current/container/noted-mobile.sqlite3");
        assert_eq!(
            reanchor_migration_recovery_path(database, Path::new("/old/container/notes.sqlite3")),
            Err("mobile migration recovery filename is invalid".to_string())
        );
    }

    #[test]
    fn fresh_unpaired_replica_allows_an_empty_native_inventory() {
        let health = MobileStoreHealth {
            storage: "ready".to_string(),
            sync: "local".to_string(),
        };
        let requirement =
            native_identity_requirement_from_health(&health).expect("map local replica state");

        assert_eq!(requirement, NativeIdentityRequirement::FreshUnpaired);
        assert!(
            reconcile_identity_inventory(empty_inventory(), DEVICE_ID, requirement, None).is_ok()
        );
    }

    #[test]
    fn paired_replica_with_wiped_keychain_is_rejected() {
        for sync in ["not_enrolled", "pending", "syncing", "synced", "error"] {
            let health = MobileStoreHealth {
                storage: "ready".to_string(),
                sync: sync.to_string(),
            };
            let requirement =
                native_identity_requirement_from_health(&health).expect("map paired replica state");

            assert_eq!(requirement, NativeIdentityRequirement::Active);
            assert_eq!(
                reconcile_identity_inventory(empty_inventory(), DEVICE_ID, requirement, None),
                Err("paired mobile replica requires exactly one matching active native identity; explicit recovery is required".to_string())
            );
        }
    }

    #[test]
    fn paired_replica_requires_one_matching_active_identity() {
        let matching = IdentityInventory {
            pending: Vec::new(),
            active: vec![identity(
                "018f47f2-8ee8-7a28-91eb-9b3f2619e080",
                DEVICE_ID,
                IdentityLifecycle::Active,
            )],
            discarded: vec![identity(
                "018f47f2-8ee8-7a28-91eb-9b3f2619e081",
                OTHER_DEVICE_ID,
                IdentityLifecycle::Discarded,
            )],
        };

        assert!(reconcile_identity_inventory(
            matching,
            DEVICE_ID,
            NativeIdentityRequirement::Active,
            None,
        )
        .is_ok());

        let mismatched = IdentityInventory {
            pending: Vec::new(),
            active: vec![identity(
                "018f47f2-8ee8-7a28-91eb-9b3f2619e082",
                OTHER_DEVICE_ID,
                IdentityLifecycle::Active,
            )],
            discarded: Vec::new(),
        };
        assert!(reconcile_identity_inventory(
            mismatched,
            DEVICE_ID,
            NativeIdentityRequirement::Active,
            None,
        )
        .expect_err("reject identity from another replica")
        .contains("not bound to the current mobile replica"));
    }

    #[test]
    fn durable_checkpoint_requires_the_exact_native_handle_and_public_keys() {
        let handle = "018f47f2-8ee8-7a28-91eb-9b3f2619e088";
        let expected = ExpectedNativeIdentity {
            handle: handle.to_string(),
            signing_public_key: vec![4; 65],
            hpke_public_key: vec![7; 32],
        };
        let matching = IdentityInventory {
            pending: Vec::new(),
            active: vec![identity(handle, DEVICE_ID, IdentityLifecycle::Active)],
            discarded: Vec::new(),
        };
        assert!(reconcile_identity_inventory(
            matching.clone(),
            DEVICE_ID,
            NativeIdentityRequirement::Active,
            Some(&expected),
        )
        .is_ok());

        let mut wrong_handle = matching.clone();
        wrong_handle.active[0] = identity(
            "018f47f2-8ee8-7a28-91eb-9b3f2619e089",
            DEVICE_ID,
            IdentityLifecycle::Active,
        );
        let mut wrong_signing_key = matching.clone();
        wrong_signing_key.active[0].signing_public_key = vec![5; 65];
        let mut wrong_hpke_key = matching;
        wrong_hpke_key.active[0].hpke_public_key = vec![8; 32];

        for inventory in [wrong_handle, wrong_signing_key, wrong_hpke_key] {
            assert!(reconcile_identity_inventory(
                inventory,
                DEVICE_ID,
                NativeIdentityRequirement::Active,
                Some(&expected),
            )
            .expect_err("reject native identity that diverges from the checkpoint")
            .contains("does not match the durable pairing checkpoint"));
        }
    }

    #[test]
    fn pairing_checkpoint_requirements_track_native_lifecycle() {
        let pending = IdentityInventory {
            pending: vec![identity(
                "018f47f2-8ee8-7a28-91eb-9b3f2619e086",
                DEVICE_ID,
                IdentityLifecycle::Pending,
            )],
            active: Vec::new(),
            discarded: Vec::new(),
        };
        assert!(reconcile_identity_inventory(
            pending.clone(),
            DEVICE_ID,
            NativeIdentityRequirement::PairingPending,
            None,
        )
        .is_ok());
        assert!(reconcile_identity_inventory(
            pending,
            DEVICE_ID,
            NativeIdentityRequirement::ActivationTransition,
            None,
        )
        .is_ok());

        let active = IdentityInventory {
            pending: Vec::new(),
            active: vec![identity(
                "018f47f2-8ee8-7a28-91eb-9b3f2619e087",
                DEVICE_ID,
                IdentityLifecycle::Active,
            )],
            discarded: Vec::new(),
        };
        assert!(reconcile_identity_inventory(
            active.clone(),
            DEVICE_ID,
            NativeIdentityRequirement::ActivationTransition,
            None,
        )
        .is_ok());
        assert!(reconcile_identity_inventory(
            active,
            DEVICE_ID,
            NativeIdentityRequirement::FreshUnpaired,
            None,
        )
        .is_err());
    }

    #[test]
    fn multiple_live_identities_are_rejected_while_discarded_history_is_preserved() {
        let inventory = IdentityInventory {
            pending: Vec::new(),
            active: vec![
                identity(
                    "018f47f2-8ee8-7a28-91eb-9b3f2619e083",
                    DEVICE_ID,
                    IdentityLifecycle::Active,
                ),
                identity(
                    "018f47f2-8ee8-7a28-91eb-9b3f2619e084",
                    DEVICE_ID,
                    IdentityLifecycle::Active,
                ),
            ],
            discarded: vec![identity(
                "018f47f2-8ee8-7a28-91eb-9b3f2619e085",
                OTHER_DEVICE_ID,
                IdentityLifecycle::Discarded,
            )],
        };

        assert!(reconcile_identity_inventory(
            inventory,
            DEVICE_ID,
            NativeIdentityRequirement::Active,
            None,
        )
        .expect_err("reject ambiguous live identities")
        .contains("multiple live native identities"));
    }

    #[test]
    fn stale_reconciliation_cannot_overwrite_a_new_unavailable_event() {
        let gate = ProtectedDataGate::default();
        let reconciliation_epoch = gate.epoch();

        assert_eq!(gate.begin_unavailable(), reconciliation_epoch + 1);
        assert!(!gate.publish_if_epoch_unchanged(reconciliation_epoch));
        assert!(!gate.is_available());
    }

    #[test]
    fn unavailable_after_publication_always_closes_the_gate() {
        let gate = ProtectedDataGate::default();
        let reconciliation_epoch = gate.epoch();
        assert!(gate.publish_if_epoch_unchanged(reconciliation_epoch));
        assert!(gate.is_available());

        gate.begin_unavailable();
        assert!(!gate.is_available());
    }
}

fn reconcile_app_protected_data(app: &AppHandle<Wry>, force_reopen: bool) -> Result<(), String> {
    let store = app.state::<ProtectedMobileStore>();
    store.reconcile_protected_data(app.apple_security(), force_reopen)
}

fn schedule_available_reconciliation(app: AppHandle<Wry>, force_reopen: bool) {
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = reconcile_app_protected_data(&app, force_reopen) {
            eprintln!("mobile protected-data reconciliation failed: {error}");
        }
    });
}

#[derive(Serialize)]
struct MobileHealth {
    platform: &'static str,
    storage: String,
    sync: String,
}

#[tauri::command]
fn mobile_health(store: State<'_, ProtectedMobileStore>) -> Result<MobileHealth, String> {
    let health = store.with_ready_store(MobileStore::health)?;
    Ok(MobileHealth {
        platform: "ios",
        storage: health.storage,
        sync: health.sync,
    })
}

#[tauri::command]
fn list_mobile_notes(
    store: State<'_, ProtectedMobileStore>,
    query: Option<String>,
) -> Result<Vec<MobileNote>, String> {
    store.with_ready_store(|store| store.list(query.as_deref()))
}

#[tauri::command]
fn get_mobile_notes_workspace(
    store: State<'_, ProtectedMobileStore>,
    query: Option<String>,
    view: Option<String>,
    folder_id: Option<String>,
) -> Result<MobileNotesWorkspace, String> {
    store.with_ready_store(|store| {
        store.workspace(query.as_deref(), view.as_deref(), folder_id.as_deref())
    })
}

#[tauri::command]
fn create_mobile_note(
    store: State<'_, ProtectedMobileStore>,
    title: String,
    body: String,
) -> Result<MobileNote, String> {
    store.with_ready_store(|store| store.create(&title, &body))
}

#[tauri::command]
fn update_mobile_note(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
    title: String,
    body: String,
) -> Result<MobileNote, String> {
    store.with_ready_store(|store| store.update(&record_id, &title, &body))
}

#[tauri::command]
fn delete_mobile_note(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
) -> Result<(), String> {
    store.with_ready_store(|store| store.delete(&record_id))
}

#[tauri::command]
fn trash_mobile_note(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
) -> Result<(), String> {
    store.with_ready_store(|store| store.delete(&record_id))
}

#[tauri::command]
fn restore_mobile_note(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
) -> Result<MobileNote, String> {
    store.with_ready_store(|store| store.restore(&record_id))
}

#[tauri::command]
fn file_mobile_note(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
    folder_id: String,
) -> Result<MobileWorkspaceNote, String> {
    store.with_ready_store(|store| store.file_note(&record_id, &folder_id))
}

#[tauri::command]
fn undo_mobile_note_filing(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
) -> Result<MobileWorkspaceNote, String> {
    store.with_ready_store(|store| store.undo_note_filing(&record_id))
}

#[tauri::command]
fn resolve_mobile_note_conflict(
    store: State<'_, ProtectedMobileStore>,
    record_id: String,
    resolution: String,
) -> Result<MobileWorkspaceNote, String> {
    store.with_ready_store(|store| store.resolve_note_conflict(&record_id, &resolution))
}

#[tauri::command]
fn resolve_mobile_deep_link(
    store: State<'_, ProtectedMobileStore>,
    url: String,
) -> Result<MobileDeepLink, String> {
    let link = MobileDeepLink::parse(&url).map_err(|error| error.to_string())?;
    match &link {
        MobileDeepLink::Note {
            library_id,
            record_id,
        } => store.with_ready_store(|store| store.verify_note_link(library_id, record_id))?,
    }
    Ok(link)
}

#[tauri::command]
fn export_mobile_notes(store: State<'_, ProtectedMobileStore>) -> Result<String, String> {
    store.with_ready_store(MobileStore::export_notes)
}

#[tauri::command]
fn restore_mobile_notes_export(
    store: State<'_, ProtectedMobileStore>,
    export_json: String,
) -> Result<usize, String> {
    store.with_ready_store(|store| store.restore_notes_export(&export_json))
}

/// Run one bounded direct-sync pass without exposing discovery metadata,
/// protocol messages, or authenticated pins to JavaScript. Manual input is a
/// strict numeric socket address; otherwise native Bonjour supplies address
/// hints and the durable activation remains the sole TLS authority.
#[tauri::command]
async fn mobile_sync_now(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    manual_address: Option<String>,
) -> Result<MobileNotesSyncReport, String> {
    let _direct_sync = store.direct_sync.lock().await;
    store.with_ready_store(|store| store.health().map(|_| ()))?;

    let journal = MobileStoreExactRequestJournal::new(&store.store);
    let profile = journal
        .active_sync_profile()
        .map_err(|error| error.to_string())?;
    let candidates = if let Some(address) = manual_address {
        vec![PrivateLanEndpointCandidate::parse_manual_numeric(&address)
            .map_err(|error| error.to_string())?]
    } else {
        app.apple_security()
            .discover_private_lan_endpoints(1_500)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|hint| {
                let address = hint
                    .parse()
                    .map_err(|_| "native Bonjour returned a non-numeric endpoint".to_string())?;
                PrivateLanEndpointCandidate::from_bonjour_address_hint(address)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let limits = DirectSyncLimits::default();
    let session = PrivateLanDirectSyncSession::from_authenticated_activation(
        &profile,
        candidates,
        limits.clone(),
    )
    .map_err(|error| error.to_string())?;
    let crypto = AppleMobileSyncCrypto::new(app.apple_security());
    let mut orchestrator =
        MobileNotesSyncOrchestrator::new(&store.store, &crypto, &session, limits)
            .map_err(|error| error.to_string())?;
    let report = orchestrator
        .sync_once()
        .await
        .map_err(|error| error.to_string())?;
    store.with_ready_store(|store| store.health().map(|_| ()))?;
    Ok(report)
}

fn private_lan_candidates(
    app: &AppHandle<Wry>,
    manual_address: Option<String>,
) -> Result<Vec<PrivateLanEndpointCandidate>, String> {
    if let Some(address) = manual_address {
        return Ok(vec![PrivateLanEndpointCandidate::parse_manual_numeric(
            &address,
        )
        .map_err(|error| error.to_string())?]);
    }
    app.apple_security()
        .discover_private_lan_endpoints(1_500)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|hint| {
            let address = hint
                .parse()
                .map_err(|_| "native Bonjour returned a non-numeric endpoint".to_string())?;
            PrivateLanEndpointCandidate::from_bonjour_address_hint(address)
                .map_err(|error| error.to_string())
        })
        .collect()
}

/// Perform the pre-activation pinned-TLS ClientHello exchange entirely in
/// native Rust. JavaScript receives only the confirmation display model.
#[tauri::command]
async fn mobile_pairing_connect_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    invitation_json: String,
    manual_address: Option<String>,
) -> Result<FixturePairingStatus, String> {
    let _direct_sync = store.direct_sync.lock().await;
    store.with_ready_store(|store| store.health().map(|_| ()))?;
    let invitation: Invitation = serde_json::from_str(&invitation_json)
        .map_err(|error| format!("invalid pairing invitation: {error}"))?;
    let candidates = private_lan_candidates(&app, manual_address)?;
    let session = PrivateLanPairingSession::from_authenticated_invitation(&invitation, candidates)
        .map_err(|error| error.to_string())?;
    let transport = fixture_transport(invitation.tls_spki_sha256.clone());
    let begin = store.with_ready_store(|store| {
        begin_fixture_pairing(&app, store, invitation_json.as_bytes(), &transport)
    })?;
    let client_hello = begin
        .exact_outgoing_bytes
        .ok_or_else(|| "durable ClientHello is unavailable".to_string())?;
    let response = session
        .post_exact(PairingEndpoint::ClientHello, client_hello)
        .await
        .map_err(|error| error.to_string())?;
    if response.status != 200 {
        return Err("Mac rejected the pairing request".to_string());
    }
    store.with_ready_store(|store| accept_server_hello(&app, store, &response.body, &transport))
}

/// Poll for the Mac owner's approval, stage the authenticated bootstrap, send
/// ClientFinish, and atomically activate the phone when approval is ready.
#[tauri::command]
async fn mobile_pairing_poll_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    manual_address: Option<String>,
) -> Result<FixturePairingStatus, String> {
    let _direct_sync = store.direct_sync.lock().await;
    store.with_ready_store(|store| store.health().map(|_| ()))?;
    let invitation = store.with_ready_store(|store| {
        let checkpoint = store
            .load_pairing_checkpoint()?
            .ok_or_else(|| "pairing has not started".to_string())?;
        serde_json::from_slice::<Invitation>(&checkpoint.client.invitation_bytes)
            .map_err(|error| format!("invalid durable invitation: {error}"))
    })?;
    let candidates = private_lan_candidates(&app, manual_address)?;
    let session = PrivateLanPairingSession::from_authenticated_invitation(&invitation, candidates)
        .map_err(|error| error.to_string())?;
    let transport = fixture_transport(invitation.tls_spki_sha256.clone());
    let poll_request = store.with_ready_store(|store| create_bootstrap_poll(&app, store))?;
    let poll_response = session
        .post_exact(PairingEndpoint::Bootstrap, poll_request.clone())
        .await
        .map_err(|error| error.to_string())?;
    let Some(prepared) = store.with_ready_store(|store| {
        accept_bootstrap_poll_response(&app, store, &poll_request, &poll_response.body, &transport)
    })?
    else {
        return store.with_ready_store(|store| recover_fixture_pairing(&app, store));
    };
    let client_finish = prepared
        .exact_outgoing_bytes
        .ok_or_else(|| "durable ClientFinish is unavailable".to_string())?;
    let finish_response = session
        .post_exact(PairingEndpoint::ClientFinish, client_finish)
        .await
        .map_err(|error| error.to_string())?;
    if finish_response.status != 200 {
        return Err("Mac rejected pairing activation".to_string());
    }
    store.with_ready_store(|store| {
        accept_server_finish(&app, store, &finish_response.body, &transport)
    })
}

fn fixture_transport(peer_spki_sha256: Vec<u8>) -> TransportEvidence {
    TransportEvidence {
        tls_version: "1.3".to_string(),
        used_zero_rtt: false,
        peer_spki_sha256,
    }
}

#[tauri::command]
fn mobile_pairing_status_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
) -> Result<FixturePairingStatus, String> {
    store.with_ready_store(|store| recover_fixture_pairing(&app, store))
}

#[tauri::command]
fn mobile_pairing_begin_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    invitation_json: String,
    peer_spki_sha256: Vec<u8>,
) -> Result<FixturePairingStatus, String> {
    let transport = fixture_transport(peer_spki_sha256);
    store.with_ready_store(|store| {
        begin_fixture_pairing(&app, store, invitation_json.as_bytes(), &transport)
    })
}

#[tauri::command]
fn mobile_pairing_accept_server_hello_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    server_hello_json: String,
    peer_spki_sha256: Vec<u8>,
) -> Result<FixturePairingStatus, String> {
    let transport = fixture_transport(peer_spki_sha256);
    store.with_ready_store(|store| {
        accept_server_hello(&app, store, server_hello_json.as_bytes(), &transport)
    })
}

#[tauri::command]
fn mobile_pairing_confirm_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    verification_code: String,
    approved: bool,
) -> Result<FixturePairingStatus, String> {
    store.with_ready_store(|store| {
        confirm_fixture_pairing(&app, store, &verification_code, approved)
    })
}

#[tauri::command]
fn mobile_pairing_accept_bootstrap_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    bootstrap_json: String,
    peer_spki_sha256: Vec<u8>,
) -> Result<FixturePairingStatus, String> {
    let transport = fixture_transport(peer_spki_sha256);
    store.with_ready_store(|store| {
        accept_bootstrap(&app, store, bootstrap_json.as_bytes(), &transport)
    })
}

#[tauri::command]
fn mobile_pairing_accept_server_finish_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
    server_finish_json: String,
    peer_spki_sha256: Vec<u8>,
) -> Result<FixturePairingStatus, String> {
    let transport = fixture_transport(peer_spki_sha256);
    store.with_ready_store(|store| {
        accept_server_finish(&app, store, server_finish_json.as_bytes(), &transport)
    })
}

#[tauri::command]
fn mobile_pairing_discard_fixture(
    app: AppHandle<Wry>,
    store: State<'_, ProtectedMobileStore>,
) -> Result<FixturePairingStatus, String> {
    store.with_ready_store(|store| discard_fixture_pairing(&app, store))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // The native security plugin must be registered before setup so the
        // first protected-data query happens before any SQLite open.
        .plugin(noted_apple_security::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let store = ProtectedMobileStore::closed(&data_dir.join("noted-mobile.sqlite3"));

            // This is the first operation that may open SQLite. When iOS is
            // locked, reconciliation leaves `store` in path-only state and
            // every command returns MOBILE_STORE_LOCKED_ERROR.
            store
                .reconcile_protected_data(app.apple_security(), false)
                .map_err(std::io::Error::other)?;
            app.manage(store);

            let app_handle = app.handle().clone();
            app.apple_security()
                .subscribe_protected_data(move |event: ProtectedDataEvent| match event.state {
                    ProtectedDataState::Unavailable => {
                        // Do not enqueue this transition: the callback only
                        // returns after all in-flight commands finish and the
                        // SQLite connection has been closed.
                        let store = app_handle.state::<ProtectedMobileStore>();
                        if let Err(error) = store.protected_data_became_unavailable() {
                            eprintln!("failed to close mobile protected store: {error}");
                        }
                    }
                    ProtectedDataState::Available => {
                        // The native subscription emits its current state while
                        // being installed. Re-entering the plugin from that
                        // callback could deadlock, so reopen on a blocking task.
                        schedule_available_reconciliation(app_handle.clone(), false);
                    }
                })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mobile_health,
            get_mobile_notes_workspace,
            list_mobile_notes,
            create_mobile_note,
            update_mobile_note,
            delete_mobile_note,
            trash_mobile_note,
            restore_mobile_note,
            file_mobile_note,
            undo_mobile_note_filing,
            resolve_mobile_note_conflict,
            resolve_mobile_deep_link,
            export_mobile_notes,
            restore_mobile_notes_export,
            mobile_sync_now,
            mobile_pairing_status_fixture,
            mobile_pairing_connect_fixture,
            mobile_pairing_poll_fixture,
            mobile_pairing_begin_fixture,
            mobile_pairing_accept_server_hello_fixture,
            mobile_pairing_confirm_fixture,
            mobile_pairing_accept_bootstrap_fixture,
            mobile_pairing_accept_server_finish_fixture,
            mobile_pairing_discard_fixture
        ])
        .build(tauri::generate_context!())
        .expect("error while running Noted on iOS");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Resumed) {
            // Re-query rather than trusting notification delivery. Reopening
            // also discards any SQLite handle that may have survived a missed
            // unavailable callback while the app was suspended.
            schedule_available_reconciliation(app_handle.clone(), true);
        }
    });
}
