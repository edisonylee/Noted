//! Restart-safe, sanitized-fixture-only iPhone pairing coordination.
//!
//! The portable planner is testable on the host. The concrete Apple adapter is
//! compiled only for iOS and keeps all private signing, HPKE, and bootstrap
//! material behind the native plugin boundary.

use crate::mobile_store::MobilePairingCheckpoint;
use crate::pairing_client::PairingClientState;
use crate::pairing_protocol::BootstrapEnvelope;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePairingLifecycle {
    Pending,
    Active,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBootstrapSnapshot {
    pub handle: String,
    pub receipt_id: String,
    pub envelope_digest: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeIdentitySnapshot {
    pub handle: String,
    pub device_id: String,
    pub signing_public_key: Vec<u8>,
    pub hpke_public_key: Vec<u8>,
    pub lifecycle: NativePairingLifecycle,
    pub bootstrap: Option<NativeBootstrapSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingRecoveryAction {
    None,
    Continue,
    DiscardRequired,
    StagePreparedBootstrap,
    ResumeExactClientFinish,
    ActivateVerifiedBootstrap,
    CommitCompletedActivation,
    CommitCompletedDiscard,
    Active,
    Cancelled,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRecoveryPlan {
    pub action: PairingRecoveryAction,
    pub reason: &'static str,
}

pub fn plan_pairing_recovery(
    checkpoint: Option<&MobilePairingCheckpoint>,
    inventory: &[NativeIdentitySnapshot],
) -> PairingRecoveryPlan {
    let live = inventory
        .iter()
        .filter(|identity| identity.lifecycle != NativePairingLifecycle::Discarded)
        .collect::<Vec<_>>();
    let Some(checkpoint) = checkpoint else {
        return match live.as_slice() {
            [] => plan(PairingRecoveryAction::None, "no pairing has started"),
            [identity] if identity.lifecycle == NativePairingLifecycle::Pending => plan(
                PairingRecoveryAction::DiscardRequired,
                "native identity creation completed before the first SQLite checkpoint",
            ),
            _ => plan(
                PairingRecoveryAction::Blocked,
                "native identity inventory is ambiguous without a durable checkpoint",
            ),
        };
    };

    let matching = inventory
        .iter()
        .filter(|identity| identity.handle == checkpoint.identity_handle)
        .collect::<Vec<_>>();
    let [native] = matching.as_slice() else {
        return plan(
            PairingRecoveryAction::Blocked,
            "durable pairing identity is missing or duplicated in native custody",
        );
    };
    if native.device_id != checkpoint.client.identity.device_id {
        return plan(
            PairingRecoveryAction::Blocked,
            "native identity is bound to a different SQLite replica",
        );
    }
    if native.signing_public_key != checkpoint.client.identity.signing_public_key
        || native.hpke_public_key != checkpoint.client.identity.hpke_public_key
    {
        return plan(
            PairingRecoveryAction::Blocked,
            "native public keys do not match the durable pairing identity",
        );
    }
    if live.len() > 1 {
        return plan(
            PairingRecoveryAction::Blocked,
            "multiple live native identities require an explicit recovery choice",
        );
    }
    if checkpoint.client.state == PairingClientState::Cancelled {
        return if native.lifecycle == NativePairingLifecycle::Discarded {
            plan(
                PairingRecoveryAction::Cancelled,
                "pairing was explicitly discarded",
            )
        } else if native.lifecycle == NativePairingLifecycle::Active {
            plan(
                PairingRecoveryAction::Blocked,
                "active native identity requires authenticated revocation",
            )
        } else {
            plan(
                PairingRecoveryAction::DiscardRequired,
                "cancelled SQLite state still has live native custody",
            )
        };
    }
    if native.lifecycle == NativePairingLifecycle::Discarded {
        return if !live.is_empty() {
            plan(
                PairingRecoveryAction::Blocked,
                "a completed discard cannot select between another live native identity",
            )
        } else if matches!(
            checkpoint.client.state,
            PairingClientState::PendingActivation | PairingClientState::Active
        ) {
            plan(
                PairingRecoveryAction::Blocked,
                "a verified activation can never be converted into a local discard",
            )
        } else {
            plan(
                PairingRecoveryAction::CommitCompletedDiscard,
                "native discard completed before SQLite reached Cancelled",
            )
        };
    }

    let expected_binding = checkpoint_bootstrap_binding(checkpoint);
    if let Some(native_binding) = &native.bootstrap {
        let Some((receipt_id, digest)) = expected_binding else {
            return plan(
                PairingRecoveryAction::DiscardRequired,
                "native bootstrap was staged before its exact SQLite checkpoint",
            );
        };
        if native_binding.receipt_id != receipt_id
            || native_binding.envelope_digest != digest
            || checkpoint
                .pending_bootstrap_handle
                .as_deref()
                .is_some_and(|handle| handle != native_binding.handle)
        {
            return plan(
                PairingRecoveryAction::Blocked,
                "native bootstrap receipt, digest, or opaque handle does not match SQLite",
            );
        }
    } else if checkpoint.pending_bootstrap_handle.is_some() {
        return plan(
            PairingRecoveryAction::Blocked,
            "SQLite names a pending bootstrap that native custody does not contain",
        );
    }

    match (
        checkpoint.client.state,
        native.lifecycle,
        native.bootstrap.is_some(),
    ) {
        (PairingClientState::Ready, NativePairingLifecycle::Pending, false)
        | (PairingClientState::AwaitingServerHello, NativePairingLifecycle::Pending, false)
        | (PairingClientState::AwaitingUserConfirmation, NativePairingLifecycle::Pending, false)
        | (PairingClientState::AwaitingBootstrap, NativePairingLifecycle::Pending, false) => plan(
            PairingRecoveryAction::Continue,
            "durable pairing step can continue",
        ),
        (PairingClientState::BootstrapPrepared, NativePairingLifecycle::Pending, false) => plan(
            PairingRecoveryAction::StagePreparedBootstrap,
            "exact BootstrapEnvelope and ClientFinish were saved before native staging",
        ),
        (PairingClientState::BootstrapPrepared, NativePairingLifecycle::Pending, true)
        | (PairingClientState::AwaitingServerFinish, NativePairingLifecycle::Pending, true) => {
            plan(
                PairingRecoveryAction::ResumeExactClientFinish,
                "native stage matches the exact durable ClientFinish",
            )
        }
        (PairingClientState::PendingActivation, NativePairingLifecycle::Pending, true) => plan(
            PairingRecoveryAction::ActivateVerifiedBootstrap,
            "verified ServerFinish is durable and native activation is pending",
        ),
        (PairingClientState::PendingActivation, NativePairingLifecycle::Active, true) => plan(
            PairingRecoveryAction::CommitCompletedActivation,
            "native activation completed before SQLite reached Active",
        ),
        (PairingClientState::Active, NativePairingLifecycle::Active, true) => {
            plan(PairingRecoveryAction::Active, "pairing is active")
        }
        _ => plan(
            PairingRecoveryAction::Blocked,
            "native lifecycle and durable pairing state do not form a safe recovery transition",
        ),
    }
}

pub fn checkpoint_after_completed_discard(
    checkpoint: &MobilePairingCheckpoint,
    inventory: &[NativeIdentitySnapshot],
    updated_at: i64,
) -> Option<MobilePairingCheckpoint> {
    if updated_at < 0
        || plan_pairing_recovery(Some(checkpoint), inventory).action
            != PairingRecoveryAction::CommitCompletedDiscard
    {
        return None;
    }
    let mut completed = checkpoint.clone();
    completed.client.state = PairingClientState::Cancelled;
    completed.client.user_decision = Some(false);
    completed.pending_bootstrap_handle = None;
    completed.updated_at = updated_at;
    Some(completed)
}

fn plan(action: PairingRecoveryAction, reason: &'static str) -> PairingRecoveryPlan {
    PairingRecoveryPlan { action, reason }
}

fn checkpoint_bootstrap_binding(checkpoint: &MobilePairingCheckpoint) -> Option<(String, Vec<u8>)> {
    let bytes = checkpoint.client.bootstrap_bytes.as_deref()?;
    let bootstrap: BootstrapEnvelope = serde_json::from_slice(bytes).ok()?;
    Some((bootstrap.receipt_id, bootstrap.envelope_digest))
}

#[cfg(target_os = "ios")]
mod ios {
    use super::*;
    use crate::mobile_store::MobileStore;
    use crate::pairing_client::{
        ClientFreshValuePurpose, ClientPublicIdentity, OpenedPairingChallenge, PairingActivation,
        PairingClient, PairingClientConfig, PairingClientCrypto, PairingClientError,
        PairingConfirmation,
    };
    use crate::pairing_protocol::{
        AuthenticatedHpkeEnvelope, EnrollmentReceipt, Environment, KindCapability,
        LibraryDataClass, PairingRole, RecordKind, TransportEvidence,
    };
    use noted_apple_security::{
        AppleSecurityExt, IdentityHandle, IdentityInventory, IdentityLifecycle,
        PendingBootstrapHandle, PublicIdentity,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tauri::{AppHandle, Wry};
    use zeroize::Zeroizing;

    const FIXTURE_DISPLAY_NAME: &str = "Noted iPhone Fixture";

    pub struct ApplePairingCrypto {
        app: AppHandle<Wry>,
        identity: PublicIdentity,
    }

    impl ApplePairingCrypto {
        fn new(app: AppHandle<Wry>, identity: PublicIdentity) -> Result<Self, String> {
            if identity.lifecycle == IdentityLifecycle::Discarded {
                return Err("discarded native identity cannot perform pairing".to_string());
            }
            Ok(Self { app, identity })
        }
    }

    impl PairingClientCrypto for ApplePairingCrypto {
        type PendingKeyReference = PendingBootstrapHandle;

        fn public_identity(&self) -> Result<ClientPublicIdentity, ()> {
            Ok(ClientPublicIdentity {
                device_id: self.identity.device_id.clone(),
                signing_public_key: self.identity.signing_public_key.clone(),
                hpke_public_key: self.identity.hpke_public_key.clone(),
            })
        }

        fn verify_signature(
            &self,
            _signer_role: PairingRole,
            public_key: &[u8],
            message: &[u8],
            signature: &[u8],
        ) -> Result<(), ()> {
            match self
                .app
                .apple_security()
                .verify_p256_signature(public_key, message, signature)
            {
                Ok(true) => Ok(()),
                _ => Err(()),
            }
        }

        fn sign_device(&self, message: &[u8]) -> Result<Vec<u8>, ()> {
            self.app
                .apple_security()
                .sign(&self.identity.handle, message)
                .map_err(|_| ())
        }

        fn open_challenge_authenticated(
            &self,
            sender_public_key: &[u8],
            info: &[u8],
            associated_data: &[u8],
            envelope: &AuthenticatedHpkeEnvelope,
            exporter_context: &[u8],
        ) -> Result<OpenedPairingChallenge, ()> {
            let opened = self
                .app
                .apple_security()
                .open_authenticated_hpke(
                    &self.identity.handle,
                    sender_public_key,
                    info,
                    associated_data,
                    &envelope.encapsulated_key,
                    &envelope.ciphertext,
                    exporter_context,
                )
                .map_err(|_| ())?;
            Ok(OpenedPairingChallenge {
                plaintext: Zeroizing::new(opened.plaintext),
                exporter_secret: Zeroizing::new(opened.exporter_secret),
            })
        }

        fn stage_bootstrap_authenticated(
            &self,
            sender_public_key: &[u8],
            info: &[u8],
            associated_data: &[u8],
            envelope: &AuthenticatedHpkeEnvelope,
            receipt: &EnrollmentReceipt,
            envelope_digest: &[u8],
        ) -> Result<Self::PendingKeyReference, ()> {
            self.app
                .apple_security()
                .stage_bootstrap_authenticated(
                    &self.identity.handle,
                    sender_public_key,
                    info,
                    associated_data,
                    &envelope.encapsulated_key,
                    &envelope.ciphertext,
                    &receipt.receipt_id,
                    envelope_digest,
                )
                .map_err(|_| ())
        }

        fn activate_pending_bootstrap(
            &self,
            pending: &Self::PendingKeyReference,
            receipt: &EnrollmentReceipt,
        ) -> Result<(), ()> {
            let activated = self
                .app
                .apple_security()
                .activate_bootstrap(&self.identity.handle, pending, &receipt.receipt_id)
                .map_err(|_| ())?;
            if activated.device_id == self.identity.device_id
                && activated.lifecycle == IdentityLifecycle::Active
            {
                Ok(())
            } else {
                Err(())
            }
        }

        fn discard_pending_bootstrap(&self, pending: &Self::PendingKeyReference) -> Result<(), ()> {
            let receipt = self
                .identity
                .bootstrap_recovery
                .as_ref()
                .filter(|binding| binding.pending_bootstrap_handle == *pending)
                .ok_or(())?;
            self.app
                .apple_security()
                .discard_pending(
                    &self.identity.handle,
                    Some(pending),
                    Some(&receipt.receipt_id),
                )
                .map(|_| ())
                .map_err(|_| ())
        }

        fn fresh_bytes(
            &self,
            _purpose: ClientFreshValuePurpose,
            length: usize,
        ) -> Result<Vec<u8>, ()> {
            self.app
                .apple_security()
                .fresh_bytes(length)
                .map_err(|_| ())
        }

        fn fresh_uuid_v7(&self, _purpose: ClientFreshValuePurpose) -> Result<String, ()> {
            self.app.apple_security().fresh_uuid_v7().map_err(|_| ())
        }
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FixturePairingStatus {
        pub state: String,
        pub recovery_action: PairingRecoveryAction,
        pub recovery_reason: String,
        pub confirmation: Option<PairingConfirmation>,
        pub exact_outgoing_bytes: Option<Vec<u8>>,
        pub activation: Option<PairingActivation>,
    }

    pub fn begin_fixture_pairing(
        app: &AppHandle<Wry>,
        store: &MobileStore,
        invitation_bytes: &[u8],
        transport: &TransportEvidence,
    ) -> Result<FixturePairingStatus, String> {
        let inventory = app
            .apple_security()
            .identity_inventory()
            .map_err(|error| error.to_string())?;
        if let Some(row) = store
            .load_pairing_checkpoint()?
            .filter(|checkpoint| checkpoint.client.state != PairingClientState::Cancelled)
        {
            if row.client.invitation_bytes != invitation_bytes {
                return Err(
                    "a different invitation cannot replace the durable pairing transcript"
                        .to_string(),
                );
            }
            let identity = find_identity(&inventory, &row.identity_handle)
                .ok_or_else(|| "durable native identity is unavailable".to_string())?;
            let mut client = restore_client(app, &row, identity.clone(), None)?;
            let outgoing = match client.state() {
                PairingClientState::Ready => {
                    let bytes = client
                        .create_client_hello(transport)
                        .map_err(|error| error.to_string())?;
                    persist(store, &client, &identity.handle, None)?;
                    bytes
                }
                PairingClientState::AwaitingServerHello => client
                    .retry_client_hello()
                    .map_err(|error| error.to_string())?,
                _ => {
                    return Err(
                        "durable fixture pairing has already advanced past ClientHello".to_string(),
                    )
                }
            };
            return status_from_client(
                &client,
                PairingRecoveryAction::Continue,
                "the exact durable ClientHello is ready for transport",
                Some(outgoing),
            );
        }
        if inventory
            .pending
            .iter()
            .chain(inventory.active.iter())
            .next()
            .is_some()
        {
            return Err(
                "live native identity requires recovery or explicit discard before pairing"
                    .to_string(),
            );
        }
        let device_id = store.replica_device_id()?;
        let identity = prepare_fixture_identity(app, &device_id)?;
        let crypto = ApplePairingCrypto::new(app.clone(), identity.clone())?;
        let mut client = PairingClient::new_fixture_only(
            crypto,
            fixture_config(),
            invitation_bytes,
            None,
            now_ms()?,
        )
        .map_err(|error| error.to_string())?;
        persist(store, &client, &identity.handle, None)?;
        let outgoing = client
            .create_client_hello(transport)
            .map_err(|error| error.to_string())?;
        persist(store, &client, &identity.handle, None)?;
        status_from_client(
            &client,
            PairingRecoveryAction::Continue,
            "ClientHello is durably checkpointed",
            Some(outgoing),
        )
    }

    pub fn accept_server_hello(
        app: &AppHandle<Wry>,
        store: &MobileStore,
        bytes: &[u8],
        transport: &TransportEvidence,
    ) -> Result<FixturePairingStatus, String> {
        let (row, identity, pending) = load_runtime(app, store)?;
        let mut client = restore_client(app, &row, identity.clone(), pending)?;
        let confirmation = client
            .process_server_hello(bytes, None, transport, now_ms()?)
            .map_err(|error| error.to_string())?;
        persist(store, &client, &identity.handle, None)?;
        Ok(FixturePairingStatus {
            state: state_name(client.state()),
            recovery_action: PairingRecoveryAction::Continue,
            recovery_reason: "ServerHello and confirmation are durable".to_string(),
            confirmation: Some(confirmation),
            exact_outgoing_bytes: None,
            activation: None,
        })
    }

    pub fn confirm_fixture_pairing(
        app: &AppHandle<Wry>,
        store: &MobileStore,
        verification_code: &str,
        approved: bool,
    ) -> Result<FixturePairingStatus, String> {
        let (row, identity, pending) = load_runtime(app, store)?;
        let mut client = restore_client(app, &row, identity.clone(), pending)?;
        let scopes = client
            .confirmation()
            .ok_or_else(|| "pairing confirmation is unavailable".to_string())?
            .granted_scopes
            .clone();
        let result = client.confirm_on_device(verification_code, &scopes, approved);
        let terminal_rejection = !approved
            || matches!(
                result,
                Err(PairingClientError::Protocol(
                    crate::pairing_protocol::PairingError::VerificationMismatch
                        | crate::pairing_protocol::PairingError::EnrollmentCancelled
                ))
            );
        if terminal_rejection {
            app.apple_security()
                .discard_pending(&identity.handle, None, None)
                .map_err(|error| error.to_string())?;
            persist(store, &client, &identity.handle, None)?;
            return result
                .map(|_| unreachable!())
                .map_err(|error| error.to_string());
        }
        result.map_err(|error| error.to_string())?;
        persist(store, &client, &identity.handle, None)?;
        status_from_client(
            &client,
            PairingRecoveryAction::Continue,
            "the local user decision is durable",
            None,
        )
    }

    pub fn accept_bootstrap(
        app: &AppHandle<Wry>,
        store: &MobileStore,
        bytes: &[u8],
        transport: &TransportEvidence,
    ) -> Result<FixturePairingStatus, String> {
        let (row, identity, pending) = load_runtime(app, store)?;
        let mut client = restore_client(app, &row, identity.clone(), pending)?;
        let outgoing = client
            .prepare_bootstrap(bytes, None, transport, now_ms()?)
            .map_err(|error| error.to_string())?;
        // Commit exact incoming/outgoing bytes before native decryption.
        persist(store, &client, &identity.handle, None)?;
        client
            .stage_prepared_bootstrap()
            .map_err(|error| error.to_string())?;
        let pending = client
            .pending_key_reference()
            .ok_or_else(|| "native bootstrap handle is unavailable".to_string())?;
        persist(store, &client, &identity.handle, Some(pending))?;
        status_from_client(
            &client,
            PairingRecoveryAction::ResumeExactClientFinish,
            "native bootstrap and exact ClientFinish are durable",
            Some(outgoing),
        )
    }

    pub fn accept_server_finish(
        app: &AppHandle<Wry>,
        store: &MobileStore,
        bytes: &[u8],
        transport: &TransportEvidence,
    ) -> Result<FixturePairingStatus, String> {
        let (row, identity, pending) = load_runtime(app, store)?;
        let mut client = restore_client(app, &row, identity.clone(), pending)?;
        client
            .prepare_server_finish(bytes, None, transport, now_ms()?)
            .map_err(|error| error.to_string())?;
        if client.state() == PairingClientState::Active {
            let activation = client
                .retry_activation()
                .map_err(|error| error.to_string())?;
            persist(store, &client, &identity.handle, None)?;
            return status_from_client(
                &client,
                PairingRecoveryAction::Active,
                "the exact ServerFinish replay matches the stored activation",
                None,
            )
            .map(|mut status| {
                status.activation = Some(activation);
                status
            });
        }
        let pending = client
            .pending_key_reference()
            .ok_or_else(|| "native bootstrap handle is unavailable".to_string())?;
        persist(store, &client, &identity.handle, Some(pending))?;
        let activation = client
            .retry_activation()
            .map_err(|error| error.to_string())?;
        persist(store, &client, &identity.handle, None)?;
        status_from_client(
            &client,
            PairingRecoveryAction::Active,
            "native activation and SQLite checkpoint are complete",
            None,
        )
        .map(|mut status| {
            status.activation = Some(activation);
            status
        })
    }

    pub fn recover_fixture_pairing(
        app: &AppHandle<Wry>,
        store: &MobileStore,
    ) -> Result<FixturePairingStatus, String> {
        let checkpoint = store.load_pairing_checkpoint()?;
        let inventory = app
            .apple_security()
            .identity_inventory()
            .map_err(|error| error.to_string())?;
        let snapshots = inventory_snapshots(&inventory);
        let plan = plan_pairing_recovery(checkpoint.as_ref(), &snapshots);
        let Some(row) = checkpoint else {
            return Ok(status_without_client(plan));
        };
        let identity = find_identity(&inventory, &row.identity_handle)
            .ok_or_else(|| plan.reason.to_string())?;
        match plan.action {
            PairingRecoveryAction::StagePreparedBootstrap => {
                let mut client = restore_client(app, &row, identity.clone(), None)?;
                let outgoing = client
                    .stage_prepared_bootstrap()
                    .map_err(|error| error.to_string())?;
                let pending = client
                    .pending_key_reference()
                    .ok_or_else(|| "native bootstrap handle is unavailable".to_string())?;
                persist(store, &client, &identity.handle, Some(pending))?;
                status_from_client(&client, plan.action, plan.reason, Some(outgoing))
            }
            PairingRecoveryAction::ResumeExactClientFinish => {
                let pending = native_pending_handle(&identity)?;
                let client = if row.client.state == PairingClientState::BootstrapPrepared {
                    let mut client = restore_client(app, &row, identity.clone(), None)?;
                    let outgoing = client
                        .resume_staged_bootstrap(pending.clone())
                        .map_err(|error| error.to_string())?;
                    persist(store, &client, &identity.handle, Some(&pending))?;
                    return status_from_client(&client, plan.action, plan.reason, Some(outgoing));
                } else {
                    restore_client(app, &row, identity.clone(), Some(pending.clone()))?
                };
                let outgoing = client
                    .retry_client_finish()
                    .map_err(|error| error.to_string())?;
                persist(store, &client, &identity.handle, Some(&pending))?;
                status_from_client(&client, plan.action, plan.reason, Some(outgoing))
            }
            PairingRecoveryAction::ActivateVerifiedBootstrap
            | PairingRecoveryAction::CommitCompletedActivation => {
                let pending = native_pending_handle(&identity)?;
                let mut client = restore_client(app, &row, identity.clone(), Some(pending))?;
                let activation = client
                    .retry_activation()
                    .map_err(|error| error.to_string())?;
                persist(store, &client, &identity.handle, None)?;
                status_from_client(&client, PairingRecoveryAction::Active, plan.reason, None).map(
                    |mut status| {
                        status.activation = Some(activation);
                        status
                    },
                )
            }
            PairingRecoveryAction::CommitCompletedDiscard => {
                let completed = checkpoint_after_completed_discard(&row, &snapshots, now_ms()?)
                    .ok_or_else(|| "native discard recovery binding changed".to_string())?;
                store.save_pairing_checkpoint(&completed)?;
                Ok(status_without_client(super::plan(
                    PairingRecoveryAction::Cancelled,
                    plan.reason,
                )))
            }
            PairingRecoveryAction::Blocked | PairingRecoveryAction::DiscardRequired => {
                Err(plan.reason.to_string())
            }
            _ => {
                let pending = pending_for_state(row.client.state, &identity);
                let client = restore_client(app, &row, identity, pending)?;
                let outgoing = if client.state() == PairingClientState::AwaitingServerHello {
                    Some(
                        client
                            .retry_client_hello()
                            .map_err(|error| error.to_string())?,
                    )
                } else {
                    None
                };
                status_from_client(&client, plan.action, plan.reason, outgoing)
            }
        }
    }

    pub fn discard_fixture_pairing(
        app: &AppHandle<Wry>,
        store: &MobileStore,
    ) -> Result<FixturePairingStatus, String> {
        let checkpoint = store.load_pairing_checkpoint()?;
        if checkpoint.as_ref().is_some_and(|checkpoint| {
            matches!(
                checkpoint.client.state,
                PairingClientState::PendingActivation | PairingClientState::Active
            )
        }) {
            return Err(
                "verified server activation requires authenticated revocation, not local discard"
                    .to_string(),
            );
        }
        let inventory = app
            .apple_security()
            .identity_inventory()
            .map_err(|error| error.to_string())?;
        let live = inventory
            .pending
            .iter()
            .chain(inventory.active.iter())
            .collect::<Vec<_>>();
        let [identity] = live.as_slice() else {
            return if live.is_empty() {
                if let Some(mut row) = checkpoint {
                    row.client.state = PairingClientState::Cancelled;
                    row.client.user_decision = Some(false);
                    row.pending_bootstrap_handle = None;
                    row.updated_at = now_ms()?;
                    store.save_pairing_checkpoint(&row)?;
                }
                Ok(status_without_client(plan(
                    PairingRecoveryAction::Cancelled,
                    "no live fixture identity remains",
                )))
            } else {
                Err("multiple live native identities require manual recovery".to_string())
            };
        };
        if identity.lifecycle == IdentityLifecycle::Active {
            return Err("active native identity requires authenticated revocation".to_string());
        }
        let binding = identity.bootstrap_recovery.as_ref();
        app.apple_security()
            .discard_pending(
                &identity.handle,
                binding.map(|value| &value.pending_bootstrap_handle),
                binding.map(|value| value.receipt_id.as_str()),
            )
            .map_err(|error| error.to_string())?;
        if let Some(mut row) = checkpoint {
            row.client.state = PairingClientState::Cancelled;
            row.client.user_decision = Some(false);
            row.pending_bootstrap_handle = None;
            row.updated_at = now_ms()?;
            store.save_pairing_checkpoint(&row)?;
        }
        Ok(status_without_client(plan(
            PairingRecoveryAction::Cancelled,
            "native fixture identity was explicitly discarded",
        )))
    }

    fn fixture_config() -> PairingClientConfig {
        PairingClientConfig {
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            requested_scopes: BTreeSet::from([RecordKind::Note]),
            capabilities: BTreeMap::from([(
                RecordKind::Note,
                KindCapability {
                    reader_version: 1,
                    writer_version: Some(1),
                },
            )]),
            display_name: FIXTURE_DISPLAY_NAME.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            build_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    fn prepare_fixture_identity(
        app: &AppHandle<Wry>,
        device_id: &str,
    ) -> Result<PublicIdentity, String> {
        #[cfg(target_abi = "sim")]
        {
            #[cfg(feature = "sanitized-development-fixtures")]
            {
                return app
                    .apple_security()
                    .prepare_sanitized_development_fixture_identity(device_id)
                    .map_err(|error| error.to_string());
            }
            #[cfg(not(feature = "sanitized-development-fixtures"))]
            {
                let _ = (app, device_id);
                return Err(
                    "simulator fixture identity support is not compiled into this build"
                        .to_string(),
                );
            }
        }
        #[cfg(not(target_abi = "sim"))]
        {
            // Physical devices always pass no fixture gate and therefore must
            // produce a Secure Enclave identity.
            app.apple_security()
                .prepare_identity(device_id)
                .map_err(|error| error.to_string())
        }
    }

    fn load_runtime(
        app: &AppHandle<Wry>,
        store: &MobileStore,
    ) -> Result<
        (
            MobilePairingCheckpoint,
            PublicIdentity,
            Option<PendingBootstrapHandle>,
        ),
        String,
    > {
        let row = store
            .load_pairing_checkpoint()?
            .ok_or_else(|| "no durable fixture pairing exists".to_string())?;
        let inventory = app
            .apple_security()
            .identity_inventory()
            .map_err(|error| error.to_string())?;
        let identity = find_identity(&inventory, &row.identity_handle)
            .ok_or_else(|| "durable native identity is unavailable".to_string())?;
        let pending = pending_for_state(row.client.state, &identity);
        Ok((row, identity, pending))
    }

    fn restore_client(
        app: &AppHandle<Wry>,
        row: &MobilePairingCheckpoint,
        identity: PublicIdentity,
        pending: Option<PendingBootstrapHandle>,
    ) -> Result<PairingClient<ApplePairingCrypto>, String> {
        let crypto = ApplePairingCrypto::new(app.clone(), identity)?;
        PairingClient::restore_fixture_only(crypto, row.client.clone(), pending, now_ms()?)
            .map_err(|error| error.to_string())
    }

    fn persist(
        store: &MobileStore,
        client: &PairingClient<ApplePairingCrypto>,
        identity: &IdentityHandle,
        pending: Option<&PendingBootstrapHandle>,
    ) -> Result<(), String> {
        store.save_pairing_checkpoint(&MobilePairingCheckpoint {
            identity_handle: identity.expose_opaque().to_string(),
            pending_bootstrap_handle: pending.map(|value| value.expose_opaque().to_string()),
            client: client.checkpoint(),
            updated_at: now_ms()?,
        })
    }

    fn find_identity(inventory: &IdentityInventory, handle: &str) -> Option<PublicIdentity> {
        inventory
            .pending
            .iter()
            .chain(inventory.active.iter())
            .chain(inventory.discarded.iter())
            .find(|identity| identity.handle.expose_opaque() == handle)
            .cloned()
    }

    fn native_pending_handle(identity: &PublicIdentity) -> Result<PendingBootstrapHandle, String> {
        identity
            .bootstrap_recovery
            .as_ref()
            .map(|binding| binding.pending_bootstrap_handle.clone())
            .ok_or_else(|| "native bootstrap recovery binding is unavailable".to_string())
    }

    fn pending_for_state(
        state: PairingClientState,
        identity: &PublicIdentity,
    ) -> Option<PendingBootstrapHandle> {
        if matches!(
            state,
            PairingClientState::AwaitingServerFinish | PairingClientState::PendingActivation
        ) {
            identity
                .bootstrap_recovery
                .as_ref()
                .map(|binding| binding.pending_bootstrap_handle.clone())
        } else {
            None
        }
    }

    fn inventory_snapshots(inventory: &IdentityInventory) -> Vec<NativeIdentitySnapshot> {
        inventory
            .pending
            .iter()
            .chain(inventory.active.iter())
            .chain(inventory.discarded.iter())
            .map(|identity| NativeIdentitySnapshot {
                handle: identity.handle.expose_opaque().to_string(),
                device_id: identity.device_id.clone(),
                signing_public_key: identity.signing_public_key.clone(),
                hpke_public_key: identity.hpke_public_key.clone(),
                lifecycle: match identity.lifecycle {
                    IdentityLifecycle::Pending => NativePairingLifecycle::Pending,
                    IdentityLifecycle::Active => NativePairingLifecycle::Active,
                    IdentityLifecycle::Discarded => NativePairingLifecycle::Discarded,
                },
                bootstrap: identity.bootstrap_recovery.as_ref().map(|binding| {
                    NativeBootstrapSnapshot {
                        handle: binding.pending_bootstrap_handle.expose_opaque().to_string(),
                        receipt_id: binding.receipt_id.clone(),
                        envelope_digest: binding.envelope_digest.clone(),
                    }
                }),
            })
            .collect()
    }

    fn status_from_client(
        client: &PairingClient<ApplePairingCrypto>,
        recovery_action: PairingRecoveryAction,
        reason: &str,
        exact_outgoing_bytes: Option<Vec<u8>>,
    ) -> Result<FixturePairingStatus, String> {
        Ok(FixturePairingStatus {
            state: state_name(client.state()),
            recovery_action,
            recovery_reason: reason.to_string(),
            confirmation: client.confirmation().cloned(),
            exact_outgoing_bytes,
            activation: client.activation().cloned(),
        })
    }

    fn status_without_client(plan: PairingRecoveryPlan) -> FixturePairingStatus {
        FixturePairingStatus {
            state: match plan.action {
                PairingRecoveryAction::None => "not_started".to_string(),
                PairingRecoveryAction::Cancelled => "cancelled".to_string(),
                _ => "recovery_required".to_string(),
            },
            recovery_action: plan.action,
            recovery_reason: plan.reason.to_string(),
            confirmation: None,
            exact_outgoing_bytes: None,
            activation: None,
        }
    }

    fn state_name(state: PairingClientState) -> String {
        serde_json::to_value(state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "invalid".to_string())
    }

    fn now_ms() -> Result<i64, String> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis();
        i64::try_from(millis).map_err(|_| "system clock exceeds i64 milliseconds".to_string())
    }
}

#[cfg(target_os = "ios")]
pub use ios::{
    accept_bootstrap, accept_server_finish, accept_server_hello, begin_fixture_pairing,
    confirm_fixture_pairing, discard_fixture_pairing, recover_fixture_pairing,
    FixturePairingStatus,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing_client::{
        ClientPublicIdentity, PairingClientCheckpoint, PairingClientConfig,
    };
    use crate::pairing_protocol::{Environment, KindCapability, LibraryDataClass, RecordKind};
    use std::collections::{BTreeMap, BTreeSet};

    const DEVICE: &str = "018f47a0-7b80-7000-8000-000000000001";
    const IDENTITY: &str = "018f47a0-7b80-4000-8000-000000000002";
    const PENDING: &str = "018f47a0-7b80-4000-8000-000000000003";
    const RECEIPT: &str = "018f47a0-7b80-7000-8000-000000000004";

    fn checkpoint(state: PairingClientState, with_bootstrap: bool) -> MobilePairingCheckpoint {
        let bootstrap_bytes = with_bootstrap.then(|| {
            serde_json::to_vec(&BootstrapEnvelope {
                protocol: "noted.pairing.v1".to_string(),
                receipt_id: RECEIPT.to_string(),
                sealed_bootstrap: crate::pairing_protocol::AuthenticatedHpkeEnvelope {
                    encapsulated_key: vec![1; 32],
                    ciphertext: vec![2; 16],
                },
                envelope_digest: vec![3; 32],
            })
            .unwrap()
        });
        MobilePairingCheckpoint {
            identity_handle: IDENTITY.to_string(),
            pending_bootstrap_handle: matches!(
                state,
                PairingClientState::AwaitingServerFinish | PairingClientState::PendingActivation
            )
            .then(|| PENDING.to_string()),
            client: PairingClientCheckpoint {
                version: 1,
                config: PairingClientConfig {
                    environment: Environment::Development,
                    library_data_class: LibraryDataClass::SanitizedFixture,
                    requested_scopes: BTreeSet::from([RecordKind::Note]),
                    capabilities: BTreeMap::from([(
                        RecordKind::Note,
                        KindCapability {
                            reader_version: 1,
                            writer_version: Some(1),
                        },
                    )]),
                    display_name: "Fixture".to_string(),
                    app_version: "1".to_string(),
                    build_version: "1".to_string(),
                },
                state,
                invitation_bytes: vec![b'{', b'}'],
                identity: ClientPublicIdentity {
                    device_id: DEVICE.to_string(),
                    signing_public_key: vec![4; 65],
                    hpke_public_key: vec![5; 32],
                },
                client_hello_bytes: None,
                server_hello_bytes: None,
                confirmation: None,
                user_decision: Some(true),
                bootstrap_bytes,
                client_finish_bytes: with_bootstrap.then(|| vec![b'{', b'}']),
                server_finish_bytes: None,
                activation: None,
            },
            updated_at: 1,
        }
    }

    fn native(lifecycle: NativePairingLifecycle, bootstrap: bool) -> NativeIdentitySnapshot {
        NativeIdentitySnapshot {
            handle: IDENTITY.to_string(),
            device_id: DEVICE.to_string(),
            signing_public_key: vec![4; 65],
            hpke_public_key: vec![5; 32],
            lifecycle,
            bootstrap: bootstrap.then(|| NativeBootstrapSnapshot {
                handle: PENDING.to_string(),
                receipt_id: RECEIPT.to_string(),
                envelope_digest: vec![3; 32],
            }),
        }
    }

    #[test]
    fn orphan_after_identity_creation_requires_explicit_discard() {
        let plan = plan_pairing_recovery(None, &[native(NativePairingLifecycle::Pending, false)]);
        assert_eq!(plan.action, PairingRecoveryAction::DiscardRequired);
    }

    #[test]
    fn crash_after_native_stage_resumes_exact_client_finish() {
        let row = checkpoint(PairingClientState::BootstrapPrepared, true);
        let plan =
            plan_pairing_recovery(Some(&row), &[native(NativePairingLifecycle::Pending, true)]);
        assert_eq!(plan.action, PairingRecoveryAction::ResumeExactClientFinish);
    }

    #[test]
    fn crash_before_native_stage_repeats_only_the_prepared_stage() {
        let row = checkpoint(PairingClientState::BootstrapPrepared, true);
        let plan = plan_pairing_recovery(
            Some(&row),
            &[native(NativePairingLifecycle::Pending, false)],
        );
        assert_eq!(plan.action, PairingRecoveryAction::StagePreparedBootstrap);
    }

    #[test]
    fn server_finish_crashes_resume_activation_or_its_sqlite_commit() {
        let row = checkpoint(PairingClientState::PendingActivation, true);
        assert_eq!(
            plan_pairing_recovery(Some(&row), &[native(NativePairingLifecycle::Pending, true)])
                .action,
            PairingRecoveryAction::ActivateVerifiedBootstrap
        );
        assert_eq!(
            plan_pairing_recovery(Some(&row), &[native(NativePairingLifecycle::Active, true)])
                .action,
            PairingRecoveryAction::CommitCompletedActivation
        );
    }

    #[test]
    fn byte_different_native_binding_never_guesses() {
        let row = checkpoint(PairingClientState::AwaitingServerFinish, true);
        let mut identity = native(NativePairingLifecycle::Pending, true);
        identity.bootstrap.as_mut().unwrap().envelope_digest[0] ^= 1;
        assert_eq!(
            plan_pairing_recovery(Some(&row), &[identity]).action,
            PairingRecoveryAction::Blocked
        );
    }

    #[test]
    fn crash_after_native_discard_commits_cancelled_only_before_activation() {
        let preactivation = checkpoint(PairingClientState::AwaitingServerFinish, true);
        let discarded = native(NativePairingLifecycle::Discarded, false);
        assert_eq!(
            plan_pairing_recovery(Some(&preactivation), std::slice::from_ref(&discarded)).action,
            PairingRecoveryAction::CommitCompletedDiscard
        );
        let completed = checkpoint_after_completed_discard(
            &preactivation,
            std::slice::from_ref(&discarded),
            42,
        )
        .expect("commit the exact completed discard");
        assert_eq!(completed.client.state, PairingClientState::Cancelled);
        assert_eq!(completed.client.user_decision, Some(false));
        assert_eq!(completed.pending_bootstrap_handle, None);
        assert_eq!(completed.updated_at, 42);

        let mut unrelated_pending = native(NativePairingLifecycle::Pending, false);
        unrelated_pending.handle = "018f47a0-7b80-4000-8000-000000000099".to_string();
        assert_eq!(
            plan_pairing_recovery(
                Some(&preactivation),
                &[discarded.clone(), unrelated_pending],
            )
            .action,
            PairingRecoveryAction::Blocked
        );

        for state in [
            PairingClientState::PendingActivation,
            PairingClientState::Active,
        ] {
            let activated = checkpoint(state, true);
            assert_eq!(
                plan_pairing_recovery(Some(&activated), std::slice::from_ref(&discarded)).action,
                PairingRecoveryAction::Blocked
            );
            assert_eq!(
                checkpoint_after_completed_discard(
                    &activated,
                    std::slice::from_ref(&discarded),
                    43,
                ),
                None
            );
        }
    }
}
