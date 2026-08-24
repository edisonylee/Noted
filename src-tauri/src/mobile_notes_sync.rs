//! Crash-safe encrypted Notes synchronization for the native companion.
//!
//! This orchestrator is deliberately UI-, URL-, and discovery-agnostic. It
//! combines an already verified private-LAN session, the exact SQLite request
//! journal, native record custody, and the canonical mobile store. Every
//! response is durable before semantic apply, every downloaded record is
//! authenticated/decrypted before entering the protected replica, and a push
//! remains `awaiting_echo` until its accepted authority change is pulled.

use crate::direct_sync::{
    parse_bounded_direct_json, response_signing_bytes, AckRequest, AckResponse, BootstrapRequest,
    BootstrapResponse, CheckpointRequest, CheckpointResponse, DirectEndpoint, DirectSyncLimits,
    NegotiateRequest, NegotiateResponse, PullRequest, PullResponse, PushRequest, PushResponse,
    SignedSyncRequest, SignedSyncResponse, SyncCheckpoint, MAX_DIRECT_BOOTSTRAP_RECORDS,
    MAX_DIRECT_PULL_CHANGES,
};
use crate::mobile_record_crypto::{MobileRecordCrypto, MobileRecordCryptoError};
use crate::mobile_store::{
    MobileBootstrapPageDraft, MobileBootstrapRecovery, MobileCanonicalBootstrapSnapshot,
    MobileCanonicalOutboxTransactionGroup, MobileCanonicalPullChange,
    MobileDirectSyncPushDisposition, MobileStore,
};
use crate::mobile_sync_runtime::{
    validate_bootstrap_writer_signatures, validate_pull_writer_signatures, ActiveSyncProfile,
    ExactRequestJournal, ExactRequestPurpose, MobileSyncRequestActor, MobileSyncRuntimeError,
    VerifiedDirectSyncSession, VerifiedSyncResponse,
};
use crate::mobile_sync_store_adapter::MobileStoreExactRequestJournal;
use crate::pairing_protocol::RecordKind;
use crate::portable::{canonical_sha256, is_uuid_v7};
use crate::sync_protocol::{
    BootstrapSnapshot, MutationDraft, MutationOperation, NegotiatedCapabilities,
    ProtocolCapabilities, ProtocolError, ReceiptDisposition, SignedTransaction, TransactionHeader,
    TransactionReceipt, BOOTSTRAP_SNAPSHOT_VERSION, SYNC_PROTOCOL_VERSION,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_OUTBOX_GROUPS_PER_PASS: usize = 16;
const TRANSACTION_LIFETIME_MS: u64 = 5 * 60 * 1_000;
const P256_P1363_SIGNATURE_BYTES: usize = 64;

pub struct MobileNotesSyncOrchestrator<'a, C, N>
where
    C: MobileRecordCrypto + ?Sized,
    N: VerifiedDirectSyncSession + ?Sized,
{
    store: &'a MobileStore,
    crypto: &'a C,
    limits: DirectSyncLimits,
    actor: MobileSyncRequestActor<MobileStoreExactRequestJournal<'a>, &'a C, &'a N>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileNotesSyncReport {
    pub recovered_request: bool,
    pub bootstrapped: bool,
    pub bootstrap_records: usize,
    pub pushed_transactions: usize,
    pub accepted_pushes: usize,
    pub conflicted_pushes: usize,
    pub pulled_transactions: usize,
    pub pulled_records: usize,
    pub final_cursor: u64,
    pub acknowledged: bool,
}

impl<'a, C, N> MobileNotesSyncOrchestrator<'a, C, N>
where
    C: MobileRecordCrypto + ?Sized,
    N: VerifiedDirectSyncSession + ?Sized,
{
    pub fn new(
        store: &'a MobileStore,
        crypto: &'a C,
        session: &'a N,
        limits: DirectSyncLimits,
    ) -> Result<Self, MobileNotesSyncError> {
        let journal = MobileStoreExactRequestJournal::new(store);
        let actor = MobileSyncRequestActor::new(journal, crypto, session, limits.clone())?;
        Ok(Self {
            store,
            crypto,
            limits,
            actor,
        })
    }

    /// Perform one bounded foreground sync pass. Repeated calls converge and
    /// are safe after transport failure or process termination.
    pub async fn sync_once(&mut self) -> Result<MobileNotesSyncReport, MobileNotesSyncError> {
        if self
            .store
            .authority_revocation()
            .map_err(MobileNotesSyncError::Store)?
            .is_some()
        {
            self.retire_native_identity()?;
            return Err(MobileSyncRuntimeError::DeviceRevoked.into());
        }
        match self.sync_once_active().await {
            Err(MobileNotesSyncError::Runtime(MobileSyncRuntimeError::DeviceRevoked)) => {
                if self
                    .store
                    .authority_revocation()
                    .map_err(MobileNotesSyncError::Store)?
                    .is_some()
                {
                    self.retire_native_identity()?;
                    return Err(MobileSyncRuntimeError::DeviceRevoked.into());
                }
                let journaled = self
                    .actor
                    .journal()
                    .unresolved_exact_request()?
                    .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
                let response = journaled
                    .response
                    .as_ref()
                    .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
                self.actor.journal().apply_authority_revocation(
                    &journaled.request_id,
                    journaled.endpoint,
                    &response.body,
                )?;
                self.retire_native_identity()?;
                Err(MobileSyncRuntimeError::DeviceRevoked.into())
            }
            result => result,
        }
    }

    fn retire_native_identity(&self) -> Result<(), MobileNotesSyncError> {
        let profile = self.actor.journal().active_sync_profile()?;
        self.crypto
            .retire_active_identity(&profile)
            .map_err(MobileNotesSyncError::RecordCrypto)
    }

    async fn sync_once_active(&mut self) -> Result<MobileNotesSyncReport, MobileNotesSyncError> {
        let mut report = MobileNotesSyncReport::default();
        report.recovered_request = self.recover_unresolved().await?;

        let negotiated = self.negotiate().await?;
        if !self
            .store
            .canonical_initial_bootstrap_applied()
            .map_err(MobileNotesSyncError::Store)?
            || self
                .store
                .recover_bootstrap_staging()
                .map_err(MobileNotesSyncError::Store)?
                .is_some()
        {
            let records = self.bootstrap().await?;
            report.bootstrapped = true;
            report.bootstrap_records = records;
        }

        let groups = self
            .store
            .eligible_canonical_outbox_transaction_groups(MAX_OUTBOX_GROUPS_PER_PASS)
            .map_err(MobileNotesSyncError::Store)?;
        for group in groups {
            let disposition = self.push_group(group, &negotiated).await?;
            report.pushed_transactions += 1;
            match disposition {
                MobileDirectSyncPushDisposition::AcceptedAwaitingEcho => {
                    report.accepted_pushes += 1
                }
                MobileDirectSyncPushDisposition::Conflict => report.conflicted_pushes += 1,
                MobileDirectSyncPushDisposition::Rejected => {}
            }
        }

        let checkpoint = self.checkpoint().await?;
        let pulled = self
            .pull_until(checkpoint.high_water_cursor, &negotiated)
            .await?;
        report.pulled_transactions += pulled.0;
        report.pulled_records += pulled.1;

        // A second signed checkpoint closes the race between the first
        // checkpoint and the final pull. If the authority advanced again, one
        // more bounded pull catches up before acknowledgement.
        let mut final_checkpoint = self.checkpoint().await?;
        let mut cursors = self
            .store
            .canonical_sync_cursors()
            .map_err(MobileNotesSyncError::Store)?;
        if to_u64(cursors.1)? < final_checkpoint.high_water_cursor {
            let pulled = self
                .pull_until(final_checkpoint.high_water_cursor, &negotiated)
                .await?;
            report.pulled_transactions += pulled.0;
            report.pulled_records += pulled.1;
            final_checkpoint = self.checkpoint().await?;
            cursors = self
                .store
                .canonical_sync_cursors()
                .map_err(MobileNotesSyncError::Store)?;
        }
        report.final_cursor = to_u64(cursors.1)?;
        if report.final_cursor == final_checkpoint.high_water_cursor {
            self.acknowledge(&final_checkpoint).await?;
            report.acknowledged = true;
        }
        self.store
            .prune_completed_direct_sync_requests(256)
            .map_err(MobileNotesSyncError::Store)?;
        Ok(report)
    }

    async fn recover_unresolved(&mut self) -> Result<bool, MobileNotesSyncError> {
        let Some(journaled) = self.actor.journal().unresolved_exact_request()? else {
            return Ok(false);
        };
        match journaled.endpoint {
            DirectEndpoint::Negotiate => {
                let response = self
                    .actor
                    .recover::<NegotiateResponse>(DirectEndpoint::Negotiate)
                    .await?
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                self.validate_negotiated(&response.payload.negotiated)?;
                self.actor.complete_verified(&response.completion)?;
            }
            DirectEndpoint::Bootstrap => {
                let response = self
                    .actor
                    .recover::<BootstrapResponse>(DirectEndpoint::Bootstrap)
                    .await?
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                self.stage_bootstrap_response(response)?;
            }
            DirectEndpoint::Push => {
                let request: SignedSyncRequest<PushRequest> = parse_bounded_direct_json(
                    &journaled.request_body,
                    self.limits.push.request_bytes,
                )
                .map_err(|_| MobileNotesSyncError::InvalidResponse)?;
                let response = self
                    .actor
                    .recover::<PushResponse>(DirectEndpoint::Push)
                    .await?
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                self.finish_push(response, &request.payload.transaction)?;
            }
            DirectEndpoint::Pull => {
                let response = self
                    .actor
                    .recover::<PullResponse>(DirectEndpoint::Pull)
                    .await?
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                let negotiated = self.expected_negotiated()?;
                self.apply_pull_response(response, &negotiated)?;
            }
            DirectEndpoint::Checkpoint => {
                let response = self
                    .actor
                    .recover::<CheckpointResponse>(DirectEndpoint::Checkpoint)
                    .await?
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                self.validate_checkpoint(&response.payload.checkpoint)?;
                self.actor.complete_verified(&response.completion)?;
            }
            DirectEndpoint::Ack => {
                let response = self
                    .actor
                    .recover::<AckResponse>(DirectEndpoint::Ack)
                    .await?
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                self.validate_ack(&response.payload)?;
                self.actor.complete_verified(&response.completion)?;
            }
        }
        Ok(true)
    }

    async fn negotiate(&mut self) -> Result<NegotiatedCapabilities, MobileNotesSyncError> {
        let capabilities = self.protocol_capabilities()?;
        let capabilities_sha256 = canonical_value_sha256(&capabilities)?;
        let response = self
            .actor
            .begin::<_, NegotiateResponse>(
                ExactRequestPurpose::Negotiate {
                    capabilities_sha256,
                },
                NegotiateRequest { capabilities },
            )
            .await?;
        self.validate_negotiated(&response.payload.negotiated)?;
        let negotiated = response.payload.negotiated;
        self.actor.complete_verified(&response.completion)?;
        Ok(negotiated)
    }

    async fn bootstrap(&mut self) -> Result<usize, MobileNotesSyncError> {
        if let Some(recovery) = self
            .store
            .recover_bootstrap_staging()
            .map_err(MobileNotesSyncError::Store)?
        {
            if recovery.checkpoint.state == "received" {
                return self.apply_staged_bootstrap(&recovery);
            }
        }

        loop {
            let recovery = self
                .store
                .recover_bootstrap_staging()
                .map_err(MobileNotesSyncError::Store)?;
            let (checkpoint_digest, after_record_id) = recovery
                .as_ref()
                .and_then(|recovery| recovery.pages.last())
                .map(|page| {
                    (
                        Some(page.checkpoint_sha256.clone()),
                        page.next_after_record_id.clone(),
                    )
                })
                .unwrap_or((None, None));
            let requested_record_kinds = self.requested_record_kinds()?;
            let response = self
                .actor
                .begin::<_, BootstrapResponse>(
                    ExactRequestPurpose::Bootstrap {
                        requested_record_kinds: requested_record_kinds.clone(),
                        checkpoint_digest: checkpoint_digest.clone(),
                        after_record_id: after_record_id.clone(),
                        limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
                    },
                    BootstrapRequest {
                        requested_record_kinds,
                        checkpoint_digest,
                        after_record_id,
                        limit: MAX_DIRECT_BOOTSTRAP_RECORDS,
                    },
                )
                .await?;
            let has_more = response.payload.page.has_more;
            self.stage_bootstrap_response(response)?;
            if !has_more {
                let recovery = self
                    .store
                    .recover_bootstrap_staging()
                    .map_err(MobileNotesSyncError::Store)?
                    .ok_or(MobileNotesSyncError::InvalidBootstrap)?;
                return self.apply_staged_bootstrap(&recovery);
            }
        }
    }

    fn stage_bootstrap_response(
        &mut self,
        response: VerifiedSyncResponse<BootstrapResponse>,
    ) -> Result<(), MobileNotesSyncError> {
        validate_bootstrap_writer_signatures(self.crypto, &response.payload)?;
        let profile = self.profile()?;
        validate_bootstrap_page_profile(&profile, &response.payload)?;
        let existing = self
            .store
            .recover_bootstrap_staging()
            .map_err(MobileNotesSyncError::Store)?;
        let checkpoint_id = existing
            .as_ref()
            .map(|recovery| recovery.checkpoint.checkpoint_id.clone())
            .unwrap_or(self.crypto.fresh_uuid_v7()?);
        // A crash can occur after the exact page commits but before the exact
        // request journal is completed. Recovering that response must replay
        // the same page index, not append a duplicate as the next page.
        let replay_index = existing.as_ref().and_then(|recovery| {
            recovery
                .pages
                .iter()
                .position(|page| page.response_bytes == response.exact_body)
        });
        let page_index = replay_index
            .or_else(|| existing.as_ref().map(|recovery| recovery.pages.len()))
            .unwrap_or(0);
        let dependency_sha256 = existing.as_ref().and_then(|recovery| {
            page_index
                .checked_sub(1)
                .and_then(|previous| recovery.pages.get(previous))
                .map(|page| page.response_sha256.clone())
        });
        let page = &response.payload.page;
        self.store
            .stage_bootstrap_page(&MobileBootstrapPageDraft {
                checkpoint_id,
                contract_version: page.contract_version.clone(),
                checkpoint_sha256: page.checkpoint_digest.clone(),
                library_id: page.library_id.clone(),
                authority_generation: to_i64(page.authority_generation)?,
                purge_generation: to_i64(page.purge_generation)?,
                key_epoch: to_i64(page.key_epoch)?,
                page_index,
                high_water_cursor: to_i64(page.high_water_cursor)?,
                requested_after_record_id: page.requested_after_record_id.clone(),
                next_after_record_id: page.next_after_record_id.clone(),
                has_more: page.has_more,
                dependency_sha256,
                response_bytes: response.exact_body.clone(),
            })
            .map_err(MobileNotesSyncError::Store)?;
        self.actor.complete_verified(&response.completion)?;
        Ok(())
    }

    fn apply_staged_bootstrap(
        &self,
        recovery: &MobileBootstrapRecovery,
    ) -> Result<usize, MobileNotesSyncError> {
        if recovery.checkpoint.state != "received" || recovery.pages.is_empty() {
            return Err(MobileNotesSyncError::InvalidBootstrap);
        }
        let profile = self.profile()?;
        let mut bootstrap_records = Vec::new();
        let mut record_bytes = Vec::new();
        let mut seen_record_ids = BTreeSet::new();
        for (index, stored_page) in recovery.pages.iter().enumerate() {
            if stored_page.page_index != index as i64 {
                return Err(MobileNotesSyncError::InvalidBootstrap);
            }
            let response =
                self.decode_staged_bootstrap_response(&profile, &stored_page.response_bytes)?;
            let page = &response.page;
            if page.checkpoint_digest != recovery.checkpoint.checkpoint_sha256
                || page.high_water_cursor != to_u64(recovery.checkpoint.high_water_cursor)?
                || page.requested_after_record_id != stored_page.requested_after_record_id
                || page.next_after_record_id != stored_page.next_after_record_id
                || page.has_more != stored_page.has_more
            {
                return Err(MobileNotesSyncError::InvalidBootstrap);
            }
            for record in &page.records {
                if !seen_record_ids.insert(record.record_id.clone()) {
                    return Err(MobileNotesSyncError::InvalidBootstrap);
                }
                let writer_key = response
                    .writer_signing_keys
                    .get(&record.mutation.device_id)
                    .ok_or(MobileNotesSyncError::InvalidBootstrap)?;
                record_bytes.push(self.crypto.open_canonical_record(
                    &profile,
                    &record.mutation,
                    writer_key,
                )?);
                bootstrap_records.push(record.clone());
            }
        }
        let snapshot = BootstrapSnapshot {
            contract_version: recovery.checkpoint.contract_version.clone(),
            library_id: recovery.checkpoint.library_id.clone(),
            authority_generation: to_u64(recovery.checkpoint.authority_generation)?,
            purge_generation: to_u64(recovery.checkpoint.purge_generation)?,
            key_epoch: to_u64(recovery.checkpoint.key_epoch)?,
            high_water_cursor: to_u64(recovery.checkpoint.high_water_cursor)?,
            records: bootstrap_records,
            checkpoint_digest: recovery.checkpoint.checkpoint_sha256.clone(),
        };
        snapshot.validate()?;
        self.store
            .apply_canonical_bootstrap_snapshot(
                &recovery.checkpoint.checkpoint_id,
                &MobileCanonicalBootstrapSnapshot {
                    checkpoint_sha256: snapshot.checkpoint_digest,
                    record_bytes,
                },
            )
            .map_err(MobileNotesSyncError::Store)
            .map(|result| result.applied_record_count)
    }

    fn decode_staged_bootstrap_response(
        &self,
        profile: &ActiveSyncProfile,
        bytes: &[u8],
    ) -> Result<BootstrapResponse, MobileNotesSyncError> {
        let signed: SignedSyncResponse<BootstrapResponse> =
            parse_bounded_direct_json(bytes, self.limits.bootstrap.response_bytes)
                .map_err(|_| MobileNotesSyncError::InvalidBootstrap)?;
        if signed.protocol_version != SYNC_PROTOCOL_VERSION
            || !is_uuid_v7(&signed.request_id)
            || signed.library_id != profile.library_id
            || signed.device_id != profile.device_id
            || signed.authority_generation != profile.authority_generation
            || signed.signature.len() != P256_P1363_SIGNATURE_BYTES
        {
            return Err(MobileNotesSyncError::InvalidBootstrap);
        }
        let signing_bytes = response_signing_bytes(DirectEndpoint::Bootstrap, &signed)
            .map_err(|_| MobileNotesSyncError::InvalidBootstrap)?;
        if !self.crypto.verify_p256_signature(
            &profile.authority_signing_public_key,
            &signing_bytes,
            &signed.signature,
        )? {
            return Err(MobileNotesSyncError::InvalidBootstrap);
        }
        validate_bootstrap_writer_signatures(self.crypto, &signed.payload)?;
        validate_bootstrap_page_profile(profile, &signed.payload)?;
        Ok(signed.payload)
    }

    async fn push_group(
        &mut self,
        group: MobileCanonicalOutboxTransactionGroup,
        negotiated: &NegotiatedCapabilities,
    ) -> Result<MobileDirectSyncPushDisposition, MobileNotesSyncError> {
        let profile = self.profile()?;
        let counter = to_u64(
            self.store
                .next_direct_sync_push_counter()
                .map_err(MobileNotesSyncError::Store)?,
        )?;
        let mut drafts = Vec::with_capacity(group.mutations.len());
        for mutation in &group.mutations {
            let operation = match mutation.operation.as_str() {
                "create" => MutationOperation::Create,
                "update" => MutationOperation::Update,
                "delete" => MutationOperation::Delete,
                _ => return Err(MobileNotesSyncError::InvalidOutbox),
            };
            let draft = MutationDraft {
                mutation_id: mutation.mutation_id.clone(),
                operation,
                record_id: mutation.record_id.clone(),
                record_kind: mutation.record_kind.clone(),
                record_schema_version: 1,
                base_head_revision: to_u64(mutation.base_revision)?,
                base_head_version_id: mutation.base_version_id.clone(),
                proposed_revision: to_u64(mutation.proposed_revision)?,
                version_id: mutation.version_id.clone(),
                ciphertext: Vec::new(),
            };
            drafts.push(self.crypto.seal_canonical_record(
                &profile,
                draft,
                &mutation.proposed_record_bytes,
            )?);
        }
        let now = now_millis()?;
        let prepared = SignedTransaction::prepare(
            TransactionHeader {
                protocol_version: negotiated.protocol_version,
                library_id: profile.library_id.clone(),
                transaction_id: group.transaction_id,
                device_id: profile.device_id.clone(),
                device_transaction_counter: counter,
                authority_generation: profile.authority_generation,
                purge_generation: profile.purge_generation,
                key_epoch: profile.key_epoch,
            },
            drafts,
            now.checked_add(TRANSACTION_LIFETIME_MS)
                .ok_or(MobileNotesSyncError::ClockUnavailable)?,
        )?;
        let transaction = self.crypto.sign_prepared_transaction(&profile, prepared)?;
        transaction.validate(now, negotiated)?;
        let purpose = ExactRequestPurpose::Push {
            transaction_id: transaction.manifest.transaction_id.clone(),
            transaction_digest: transaction.signed_digest(),
            device_transaction_counter: counter,
        };
        let response = self
            .actor
            .begin::<_, PushResponse>(
                purpose,
                PushRequest {
                    transaction: transaction.clone(),
                },
            )
            .await?;
        self.finish_push(response, &transaction)
    }

    fn finish_push(
        &mut self,
        response: VerifiedSyncResponse<PushResponse>,
        transaction: &SignedTransaction,
    ) -> Result<MobileDirectSyncPushDisposition, MobileNotesSyncError> {
        validate_push_receipt(&response.payload.receipt, transaction)?;
        let device_revoked = matches!(
            response.payload.receipt.disposition,
            ReceiptDisposition::Rejected {
                code: crate::sync_protocol::TerminalRejection::DeviceRevoked
            }
        );
        let (disposition, error_code) = match &response.payload.receipt.disposition {
            ReceiptDisposition::Accepted { .. } => {
                (MobileDirectSyncPushDisposition::AcceptedAwaitingEcho, None)
            }
            ReceiptDisposition::Conflict { .. } => (
                MobileDirectSyncPushDisposition::Conflict,
                Some("authority_conflict"),
            ),
            ReceiptDisposition::Rejected { code } => (
                MobileDirectSyncPushDisposition::Rejected,
                Some(match code {
                    crate::sync_protocol::TerminalRejection::Expired => "transaction_expired",
                    crate::sync_protocol::TerminalRejection::DeviceRevoked => "device_revoked",
                    crate::sync_protocol::TerminalRejection::AuthorityGenerationChanged => {
                        "authority_generation_changed"
                    }
                    crate::sync_protocol::TerminalRejection::PurgeGenerationChanged => {
                        "purge_generation_changed"
                    }
                }),
            ),
        };
        self.actor
            .journal()
            .complete_push(&response.completion, disposition, error_code)?;
        if device_revoked {
            self.actor.journal().apply_authority_revocation(
                &response.completion.request_id,
                response.completion.endpoint,
                &response.exact_body,
            )?;
            self.retire_native_identity()?;
            return Err(MobileSyncRuntimeError::DeviceRevoked.into());
        }
        Ok(disposition)
    }

    async fn pull_until(
        &mut self,
        target_cursor: u64,
        negotiated: &NegotiatedCapabilities,
    ) -> Result<(usize, usize), MobileNotesSyncError> {
        let mut transaction_count = 0usize;
        let mut record_count = 0usize;
        loop {
            let (_, applied_cursor) = self
                .store
                .canonical_sync_cursors()
                .map_err(MobileNotesSyncError::Store)?;
            let cursor = to_u64(applied_cursor)?;
            if cursor >= target_cursor {
                return Ok((transaction_count, record_count));
            }
            let requested_record_kinds = self.requested_record_kinds()?;
            let response = self
                .actor
                .begin::<_, PullResponse>(
                    ExactRequestPurpose::Pull {
                        requested_cursor: cursor,
                        limit: MAX_DIRECT_PULL_CHANGES,
                        requested_record_kinds: requested_record_kinds.clone(),
                    },
                    PullRequest {
                        cursor,
                        limit: MAX_DIRECT_PULL_CHANGES,
                        requested_record_kinds,
                    },
                )
                .await?;
            let applied = self.apply_pull_response(response, negotiated)?;
            if applied.0 == 0 {
                return Err(MobileNotesSyncError::CursorStalled);
            }
            transaction_count += applied.0;
            record_count += applied.1;
        }
    }

    fn apply_pull_response(
        &mut self,
        response: VerifiedSyncResponse<PullResponse>,
        negotiated: &NegotiatedCapabilities,
    ) -> Result<(usize, usize), MobileNotesSyncError> {
        validate_pull_writer_signatures(self.crypto, &response.payload)?;
        let profile = self.profile()?;
        let page = &response.payload.page;
        let (_, current_cursor) = self
            .store
            .canonical_sync_cursors()
            .map_err(MobileNotesSyncError::Store)?;
        let durable_cursor = to_u64(current_cursor)?;
        // On recovery, earlier changes from this exact stored page may already
        // be committed. Replaying the full authenticated page is intentional;
        // the store verifies exact sequence bindings and treats them as
        // idempotent before applying the remaining suffix.
        if durable_cursor < page.requested_cursor
            || durable_cursor > page.next_cursor
            || page.next_cursor < page.requested_cursor
            || page.next_cursor > page.high_water_cursor
            || page.changes.len() > MAX_DIRECT_PULL_CHANGES as usize
        {
            return Err(MobileNotesSyncError::InvalidResponse);
        }
        let mut expected_sequence = page.requested_cursor;
        let mut record_count = 0usize;
        for change in &page.changes {
            expected_sequence = expected_sequence
                .checked_add(1)
                .ok_or(MobileNotesSyncError::InvalidResponse)?;
            if change.sequence != expected_sequence {
                return Err(MobileNotesSyncError::InvalidResponse);
            }
            validate_accepted_change(change, negotiated)?;
            let mut records = Vec::with_capacity(change.transaction.members.len());
            for envelope in &change.transaction.members {
                let writer_key = response
                    .payload
                    .writer_signing_keys
                    .get(&envelope.device_id)
                    .ok_or(MobileNotesSyncError::InvalidResponse)?;
                records.push(
                    self.crypto
                        .open_canonical_record(&profile, envelope, writer_key)?,
                );
            }
            let applied = self
                .store
                .apply_canonical_pull_change(&MobileCanonicalPullChange {
                    sequence: to_i64(change.sequence)?,
                    transaction_id: change.transaction.manifest.transaction_id.clone(),
                    transaction_digest: change.transaction_digest.clone(),
                    library_id: profile.library_id.clone(),
                    source_device_id: change.transaction.manifest.device_id.clone(),
                    authority_generation: to_i64(change.receipt.authority_generation)?,
                    purge_generation: to_i64(change.receipt.purge_generation)?,
                    record_bytes: records,
                })
                .map_err(MobileNotesSyncError::Store)?;
            if applied.state != "applied" && applied.state != "conflict" {
                return Err(MobileNotesSyncError::InvalidResponse);
            }
            record_count += applied.applied_count;
        }
        if page.next_cursor != expected_sequence
            || (page.has_more && page.next_cursor >= page.high_water_cursor)
        {
            return Err(MobileNotesSyncError::InvalidResponse);
        }
        self.actor.complete_verified(&response.completion)?;
        Ok((page.changes.len(), record_count))
    }

    async fn checkpoint(&mut self) -> Result<SyncCheckpoint, MobileNotesSyncError> {
        let (_, applied) = self
            .store
            .canonical_sync_cursors()
            .map_err(MobileNotesSyncError::Store)?;
        let known_cursor = Some(to_u64(applied)?);
        let response = self
            .actor
            .begin::<_, CheckpointResponse>(
                ExactRequestPurpose::Checkpoint { known_cursor },
                CheckpointRequest { known_cursor },
            )
            .await?;
        self.validate_checkpoint(&response.payload.checkpoint)?;
        let checkpoint = response.payload.checkpoint;
        self.actor.complete_verified(&response.completion)?;
        Ok(checkpoint)
    }

    async fn acknowledge(
        &mut self,
        checkpoint: &SyncCheckpoint,
    ) -> Result<(), MobileNotesSyncError> {
        let response = self
            .actor
            .begin::<_, AckResponse>(
                ExactRequestPurpose::Ack {
                    high_water_cursor: checkpoint.high_water_cursor,
                    checkpoint_digest: checkpoint.checkpoint_digest.clone(),
                },
                AckRequest {
                    high_water_cursor: checkpoint.high_water_cursor,
                    checkpoint_digest: checkpoint.checkpoint_digest.clone(),
                },
            )
            .await?;
        self.validate_ack(&response.payload)?;
        if response.payload.receipt.high_water_cursor != checkpoint.high_water_cursor
            || response.payload.receipt.checkpoint_digest != checkpoint.checkpoint_digest
        {
            return Err(MobileNotesSyncError::InvalidResponse);
        }
        self.actor.complete_verified(&response.completion)?;
        Ok(())
    }

    fn profile(&self) -> Result<ActiveSyncProfile, MobileNotesSyncError> {
        self.actor
            .journal()
            .active_sync_profile()
            .map_err(MobileNotesSyncError::Runtime)
    }

    fn protocol_capabilities(&self) -> Result<ProtocolCapabilities, MobileNotesSyncError> {
        let profile = self.profile()?;
        let record_kinds = profile
            .capabilities
            .iter()
            .map(|(kind, capability)| {
                (
                    record_kind_name(*kind).to_owned(),
                    crate::sync_protocol::RecordKindCapability::new(
                        capability.reader_version,
                        capability.writer_version.unwrap_or(0),
                    ),
                )
            })
            .collect();
        let capabilities =
            ProtocolCapabilities::new(SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION, record_kinds);
        capabilities.validate()?;
        Ok(capabilities)
    }

    fn expected_negotiated(&self) -> Result<NegotiatedCapabilities, MobileNotesSyncError> {
        let capabilities = self.protocol_capabilities()?;
        crate::sync_protocol::negotiate_capabilities(&capabilities, &capabilities)
            .map_err(MobileNotesSyncError::Protocol)
    }

    fn validate_negotiated(
        &self,
        negotiated: &NegotiatedCapabilities,
    ) -> Result<(), MobileNotesSyncError> {
        let expected = self.expected_negotiated()?;
        if negotiated.protocol_version != expected.protocol_version
            || negotiated.record_kinds != expected.record_kinds
            || negotiated.max_transaction_members == 0
            || negotiated.max_transaction_members > expected.max_transaction_members
            || negotiated.max_transaction_bytes == 0
            || negotiated.max_transaction_bytes > expected.max_transaction_bytes
        {
            return Err(MobileNotesSyncError::CapabilityMismatch);
        }
        Ok(())
    }

    fn requested_record_kinds(&self) -> Result<BTreeSet<String>, MobileNotesSyncError> {
        Ok(self
            .profile()?
            .granted_scopes
            .into_iter()
            .map(|kind| record_kind_name(kind).to_owned())
            .collect())
    }

    fn validate_checkpoint(&self, checkpoint: &SyncCheckpoint) -> Result<(), MobileNotesSyncError> {
        let profile = self.profile()?;
        if checkpoint.contract_version != BOOTSTRAP_SNAPSHOT_VERSION
            || checkpoint.library_id != profile.library_id
            || checkpoint.authority_generation != profile.authority_generation
            || checkpoint.purge_generation != profile.purge_generation
            || checkpoint.key_epoch != profile.key_epoch
            || !is_sha256_hex(&checkpoint.checkpoint_digest)
        {
            return Err(MobileNotesSyncError::InvalidResponse);
        }
        Ok(())
    }

    fn validate_ack(&self, response: &AckResponse) -> Result<(), MobileNotesSyncError> {
        let profile = self.profile()?;
        if response.receipt.device_id != profile.device_id
            || !is_sha256_hex(&response.receipt.checkpoint_digest)
        {
            return Err(MobileNotesSyncError::InvalidResponse);
        }
        Ok(())
    }
}

fn validate_bootstrap_page_profile(
    profile: &ActiveSyncProfile,
    response: &BootstrapResponse,
) -> Result<(), MobileNotesSyncError> {
    let page = &response.page;
    if page.contract_version != BOOTSTRAP_SNAPSHOT_VERSION
        || page.library_id != profile.library_id
        || page.authority_generation != profile.authority_generation
        || page.purge_generation != profile.purge_generation
        || page.key_epoch != profile.key_epoch
        || !is_sha256_hex(&page.checkpoint_digest)
        || page.records.len() > MAX_DIRECT_BOOTSTRAP_RECORDS as usize
        || page.next_after_record_id.as_deref()
            != page.records.last().map(|record| record.record_id.as_str())
    {
        return Err(MobileNotesSyncError::InvalidBootstrap);
    }
    Ok(())
}

fn validate_push_receipt(
    receipt: &TransactionReceipt,
    transaction: &SignedTransaction,
) -> Result<(), MobileNotesSyncError> {
    let manifest = &transaction.manifest;
    let mutation_ids = transaction
        .members
        .iter()
        .map(|member| member.mutation_id.clone())
        .collect::<Vec<_>>();
    if receipt.library_id != manifest.library_id
        || receipt.transaction_id != manifest.transaction_id
        || receipt.transaction_digest != transaction.signed_digest()
        || receipt.mutation_ids != mutation_ids
        || receipt.device_id != manifest.device_id
        || receipt.device_transaction_counter != manifest.device_transaction_counter
        || receipt.authority_generation != manifest.authority_generation
        || receipt.purge_generation != manifest.purge_generation
    {
        return Err(MobileNotesSyncError::InvalidResponse);
    }
    if let ReceiptDisposition::Accepted { advances } = &receipt.disposition {
        if advances.len() != transaction.members.len()
            || transaction.members.iter().any(|member| {
                !advances.iter().any(|advance| {
                    advance.record_id == member.record_id
                        && advance.record_kind == member.record_kind
                        && advance.record_schema_version == member.record_schema_version
                        && advance.base_revision == member.base_head_revision
                        && advance.base_version_id == member.base_head_version_id
                        && advance.revision == member.proposed_revision
                        && advance.version_id == member.version_id
                        && advance.ciphertext_hash == member.ciphertext_hash
                })
            })
        {
            return Err(MobileNotesSyncError::InvalidResponse);
        }
    }
    Ok(())
}

fn validate_accepted_change(
    change: &crate::sync_protocol::AcceptedChange,
    negotiated: &NegotiatedCapabilities,
) -> Result<(), MobileNotesSyncError> {
    change
        .transaction
        .validate(change.transaction.manifest.expires_at, negotiated)?;
    validate_push_receipt(&change.receipt, &change.transaction)?;
    if change.sequence == 0
        || change.transaction_digest != change.transaction.signed_digest()
        || change.receipt.high_water_cursor != change.sequence
        || !matches!(
            change.receipt.disposition,
            ReceiptDisposition::Accepted { .. }
        )
    {
        return Err(MobileNotesSyncError::InvalidResponse);
    }
    Ok(())
}

fn record_kind_name(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Note => "note",
        RecordKind::Category => "category",
        RecordKind::Folder => "folder",
        RecordKind::Media => "media",
    }
}

fn canonical_value_sha256(value: &impl Serialize) -> Result<String, MobileNotesSyncError> {
    let value = serde_json::to_value(value).map_err(|_| MobileNotesSyncError::InvalidResponse)?;
    Ok(canonical_sha256(&value))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn now_millis() -> Result<u64, MobileNotesSyncError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| MobileNotesSyncError::ClockUnavailable)?
        .as_millis();
    u64::try_from(millis).map_err(|_| MobileNotesSyncError::ClockUnavailable)
}

fn to_i64(value: u64) -> Result<i64, MobileNotesSyncError> {
    i64::try_from(value).map_err(|_| MobileNotesSyncError::IntegerOverflow)
}

fn to_u64(value: i64) -> Result<u64, MobileNotesSyncError> {
    u64::try_from(value).map_err(|_| MobileNotesSyncError::IntegerOverflow)
}

#[derive(Debug)]
pub enum MobileNotesSyncError {
    Runtime(MobileSyncRuntimeError),
    RecordCrypto(MobileRecordCryptoError),
    Protocol(ProtocolError),
    Store(String),
    InvalidOutbox,
    InvalidBootstrap,
    InvalidResponse,
    CapabilityMismatch,
    CursorStalled,
    ClockUnavailable,
    IntegerOverflow,
}

impl fmt::Display for MobileNotesSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Runtime(_) => "mobile direct-sync request failed",
            Self::RecordCrypto(_) => "mobile record cryptography failed",
            Self::Protocol(_) => "mobile sync protocol validation failed",
            Self::Store(_) => "mobile sync state could not be committed",
            Self::InvalidOutbox => "mobile outbox transaction is invalid",
            Self::InvalidBootstrap => "mobile bootstrap is invalid",
            Self::InvalidResponse => "mobile sync response is invalid",
            Self::CapabilityMismatch => "mobile sync capabilities do not match enrollment",
            Self::CursorStalled => "mobile sync cursor did not advance",
            Self::ClockUnavailable => "mobile sync clock is unavailable",
            Self::IntegerOverflow => "mobile sync integer exceeds the durable range",
        })
    }
}

impl std::error::Error for MobileNotesSyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(error) => Some(error),
            Self::RecordCrypto(error) => Some(error),
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

impl From<MobileSyncRuntimeError> for MobileNotesSyncError {
    fn from(value: MobileSyncRuntimeError) -> Self {
        Self::Runtime(value)
    }
}

impl From<MobileRecordCryptoError> for MobileNotesSyncError {
    fn from(value: MobileRecordCryptoError) -> Self {
        Self::RecordCrypto(value)
    }
}

impl From<ProtocolError> for MobileNotesSyncError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}
