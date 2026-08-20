//! Deterministic sequencing and convergence rules for direct and relayed sync.
//!
//! This module deliberately contains no sockets, clocks, SQLite calls, or
//! cryptography. Transport adapters authenticate signatures and ciphertext,
//! then pass the exact authenticated bytes into this state machine. Keeping the
//! authority policy pure makes duplicate, reorder, crash, and conflict behavior
//! identical on the paired Mac and a future opaque relay.

use crate::portable::{canonical_json, canonical_sha256, is_uuid, is_uuid_v7};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const SYNC_PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_MAX_TRANSACTION_MEMBERS: u32 = 128;
pub const DEFAULT_MAX_TRANSACTION_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_SIGNATURE_BYTES: usize = 512;
pub const MAX_PULL_PAGE_CHANGES: u32 = 256;
pub const BOOTSTRAP_SNAPSHOT_VERSION: &str = "noted.sync-bootstrap.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordAccess {
    Reject,
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordKindCapability {
    pub max_read_schema_version: u32,
    pub max_write_schema_version: u32,
}

impl RecordKindCapability {
    pub const fn new(max_read_schema_version: u32, max_write_schema_version: u32) -> Self {
        Self {
            max_read_schema_version,
            max_write_schema_version,
        }
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        if self.max_read_schema_version == 0 {
            return Err(ProtocolError::InvalidCapability(
                "record reader version must be positive".to_string(),
            ));
        }
        if self.max_write_schema_version > self.max_read_schema_version {
            return Err(ProtocolError::InvalidCapability(
                "a record writer cannot exceed its reader version".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolCapabilities {
    pub min_protocol_version: u32,
    pub max_protocol_version: u32,
    pub record_kinds: BTreeMap<String, RecordKindCapability>,
    pub max_transaction_members: u32,
    pub max_transaction_bytes: u64,
}

impl ProtocolCapabilities {
    pub fn new(
        min_protocol_version: u32,
        max_protocol_version: u32,
        record_kinds: BTreeMap<String, RecordKindCapability>,
    ) -> Self {
        Self {
            min_protocol_version,
            max_protocol_version,
            record_kinds,
            max_transaction_members: DEFAULT_MAX_TRANSACTION_MEMBERS,
            max_transaction_bytes: DEFAULT_MAX_TRANSACTION_BYTES,
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.min_protocol_version == 0
            || self.max_protocol_version < self.min_protocol_version
            || self.max_transaction_members == 0
            || self.max_transaction_bytes == 0
        {
            return Err(ProtocolError::InvalidCapability(
                "protocol range and transaction limits must be positive and ordered".to_string(),
            ));
        }
        for (kind, capability) in &self.record_kinds {
            if kind.trim().is_empty() {
                return Err(ProtocolError::InvalidCapability(
                    "record kind cannot be empty".to_string(),
                ));
            }
            capability.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedRecordCapability {
    pub max_read_schema_version: u32,
    pub max_write_schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedCapabilities {
    pub protocol_version: u32,
    pub record_kinds: BTreeMap<String, NegotiatedRecordCapability>,
    pub max_transaction_members: u32,
    pub max_transaction_bytes: u64,
}

impl NegotiatedCapabilities {
    pub fn access_for(&self, kind: &str, schema_version: u32) -> RecordAccess {
        if schema_version == 0 {
            return RecordAccess::Reject;
        }
        let Some(capability) = self.record_kinds.get(kind) else {
            return RecordAccess::Reject;
        };
        if capability.max_read_schema_version < schema_version {
            RecordAccess::Reject
        } else if capability.max_write_schema_version < schema_version {
            RecordAccess::ReadOnly
        } else {
            RecordAccess::ReadWrite
        }
    }
}

/// Negotiate the highest common protocol and the lossless capability for every
/// record kind understood by both sides. A zero writer version is intentionally
/// retained as read-only rather than silently promoted.
pub fn negotiate_capabilities(
    left: &ProtocolCapabilities,
    right: &ProtocolCapabilities,
) -> Result<NegotiatedCapabilities, ProtocolError> {
    left.validate()?;
    right.validate()?;
    let minimum = left.min_protocol_version.max(right.min_protocol_version);
    let maximum = left.max_protocol_version.min(right.max_protocol_version);
    if maximum < minimum {
        return Err(ProtocolError::UnsupportedProtocol);
    }

    let mut record_kinds = BTreeMap::new();
    for (kind, left_capability) in &left.record_kinds {
        let Some(right_capability) = right.record_kinds.get(kind) else {
            continue;
        };
        record_kinds.insert(
            kind.clone(),
            NegotiatedRecordCapability {
                max_read_schema_version: left_capability
                    .max_read_schema_version
                    .min(right_capability.max_read_schema_version),
                max_write_schema_version: left_capability
                    .max_write_schema_version
                    .min(right_capability.max_write_schema_version),
            },
        );
    }

    Ok(NegotiatedCapabilities {
        protocol_version: maximum,
        record_kinds,
        max_transaction_members: left
            .max_transaction_members
            .min(right.max_transaction_members),
        max_transaction_bytes: left.max_transaction_bytes.min(right.max_transaction_bytes),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionHeader {
    pub protocol_version: u32,
    pub library_id: String,
    pub transaction_id: String,
    pub device_id: String,
    pub device_transaction_counter: u64,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOperation {
    Create,
    Update,
    Delete,
}

impl MutationOperation {
    fn validates_revision_contract(
        &self,
        base_head_revision: u64,
        base_head_version_id: Option<&str>,
        proposed_revision: u64,
    ) -> bool {
        let Some(expected_revision) = base_head_revision.checked_add(1) else {
            return false;
        };
        if proposed_revision != expected_revision {
            return false;
        }
        match self {
            Self::Create => base_head_revision == 0 && base_head_version_id.is_none(),
            Self::Update | Self::Delete => base_head_revision > 0 && base_head_version_id.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationDraft {
    pub mutation_id: String,
    pub operation: MutationOperation,
    pub record_id: String,
    pub record_kind: String,
    pub record_schema_version: u32,
    pub base_head_revision: u64,
    pub base_head_version_id: Option<String>,
    pub proposed_revision: u64,
    pub version_id: String,
    pub ciphertext: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationEnvelope {
    pub protocol_version: u32,
    pub library_id: String,
    pub mutation_id: String,
    pub transaction_id: String,
    pub transaction_member_index: u32,
    pub transaction_member_count: u32,
    pub transaction_manifest_digest: String,
    pub transaction_commit_marker: bool,
    pub device_id: String,
    pub device_transaction_counter: u64,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub operation: MutationOperation,
    pub record_id: String,
    pub record_kind: String,
    pub record_schema_version: u32,
    pub base_head_revision: u64,
    pub base_head_version_id: Option<String>,
    pub proposed_revision: u64,
    pub version_id: String,
    pub key_epoch: u64,
    pub ciphertext: Vec<u8>,
    pub ciphertext_hash: String,
    pub signature: Vec<u8>,
}

impl MutationEnvelope {
    /// Digest placed in the ordered transaction manifest. It excludes the
    /// manifest digest (which would be circular) and the later signature, but
    /// includes every mutation and transaction-routing field.
    pub fn member_digest(&self) -> String {
        canonical_sha256(&json!({
            "protocol_version": self.protocol_version,
            "library_id": self.library_id,
            "mutation_id": self.mutation_id,
            "transaction_id": self.transaction_id,
            "transaction_member_index": self.transaction_member_index,
            "transaction_member_count": self.transaction_member_count,
            "transaction_commit_marker": self.transaction_commit_marker,
            "device_id": self.device_id,
            "device_transaction_counter": self.device_transaction_counter,
            "authority_generation": self.authority_generation,
            "purge_generation": self.purge_generation,
            "operation": self.operation,
            "record_id": self.record_id,
            "record_kind": self.record_kind,
            "record_schema_version": self.record_schema_version,
            "base_head_revision": self.base_head_revision,
            "base_head_version_id": self.base_head_version_id,
            "proposed_revision": self.proposed_revision,
            "version_id": self.version_id,
            "key_epoch": self.key_epoch,
            "ciphertext": self.ciphertext,
            "ciphertext_hash": self.ciphertext_hash,
        }))
    }

    /// Exact canonical bytes authenticated after the aggregate manifest has
    /// been attached. Signing bytes, rather than a textual hex digest, keeps
    /// hashing behavior explicit across CryptoKit and Rust implementations.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature.clear();
        canonical_json(&json!({
            "domain": "noted.sync.v1/mutation",
            "mutation": unsigned,
        }))
        .into_bytes()
    }

    pub fn signing_digest(&self) -> String {
        sha256_bytes(&self.signing_bytes())
    }

    /// Exact signed-envelope binding used for mutation-ID replay checks.
    pub fn signed_digest(&self) -> String {
        canonical_sha256(&serde_json::to_value(self).expect("envelope serialization cannot fail"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionManifest {
    pub protocol_version: u32,
    pub library_id: String,
    pub transaction_id: String,
    pub device_id: String,
    pub device_transaction_counter: u64,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub member_count: u32,
    pub ordered_member_digests: Vec<String>,
    pub byte_total: u64,
    /// Deterministic authority time/tick supplied by the adapter.
    pub expires_at: u64,
}

impl TransactionManifest {
    pub fn digest(&self) -> String {
        canonical_sha256(&serde_json::to_value(self).expect("manifest serialization cannot fail"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTransaction {
    pub manifest: TransactionManifest,
    pub members: Vec<MutationEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationSigningInput {
    pub mutation_id: String,
    pub member_index: u32,
    pub canonical_bytes: Vec<u8>,
}

/// A transaction whose final manifest is frozen but whose member signatures
/// have not yet been attached. This makes the signable bytes available to a
/// native key provider without permitting signatures over a placeholder
/// manifest digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTransaction {
    manifest: TransactionManifest,
    members: Vec<MutationEnvelope>,
}

impl PreparedTransaction {
    pub fn prepare(
        header: TransactionHeader,
        drafts: Vec<MutationDraft>,
        expires_at: u64,
    ) -> Result<Self, ProtocolError> {
        if drafts.is_empty() {
            return Err(ProtocolError::IncompleteTransaction);
        }
        let member_count =
            u32::try_from(drafts.len()).map_err(|_| ProtocolError::TransactionLimitExceeded)?;
        let mut members = Vec::with_capacity(drafts.len());
        for (index, draft) in drafts.into_iter().enumerate() {
            if !draft.operation.validates_revision_contract(
                draft.base_head_revision,
                draft.base_head_version_id.as_deref(),
                draft.proposed_revision,
            ) {
                return Err(ProtocolError::MalformedEnvelope);
            }
            let ciphertext_hash = sha256_bytes(&draft.ciphertext);
            members.push(MutationEnvelope {
                protocol_version: header.protocol_version,
                library_id: header.library_id.clone(),
                mutation_id: draft.mutation_id,
                transaction_id: header.transaction_id.clone(),
                transaction_member_index: index as u32,
                transaction_member_count: member_count,
                transaction_manifest_digest: String::new(),
                transaction_commit_marker: index + 1 == member_count as usize,
                device_id: header.device_id.clone(),
                device_transaction_counter: header.device_transaction_counter,
                authority_generation: header.authority_generation,
                purge_generation: header.purge_generation,
                operation: draft.operation,
                record_id: draft.record_id,
                record_kind: draft.record_kind,
                record_schema_version: draft.record_schema_version,
                base_head_revision: draft.base_head_revision,
                base_head_version_id: draft.base_head_version_id,
                proposed_revision: draft.proposed_revision,
                version_id: draft.version_id,
                key_epoch: header.key_epoch,
                ciphertext: draft.ciphertext,
                ciphertext_hash,
                signature: Vec::new(),
            });
        }
        let byte_total = members.iter().try_fold(0_u64, |total, member| {
            total
                .checked_add(member.ciphertext.len() as u64)
                .ok_or(ProtocolError::TransactionLimitExceeded)
        })?;
        let manifest = TransactionManifest {
            protocol_version: header.protocol_version,
            library_id: header.library_id,
            transaction_id: header.transaction_id,
            device_id: header.device_id,
            device_transaction_counter: header.device_transaction_counter,
            authority_generation: header.authority_generation,
            purge_generation: header.purge_generation,
            key_epoch: header.key_epoch,
            member_count,
            ordered_member_digests: members
                .iter()
                .map(MutationEnvelope::member_digest)
                .collect(),
            byte_total,
            expires_at,
        };
        let manifest_digest = manifest.digest();
        for member in &mut members {
            member.transaction_manifest_digest = manifest_digest.clone();
        }
        Ok(Self { manifest, members })
    }

    pub fn signing_inputs(&self) -> Vec<MutationSigningInput> {
        self.members
            .iter()
            .map(|member| MutationSigningInput {
                mutation_id: member.mutation_id.clone(),
                member_index: member.transaction_member_index,
                canonical_bytes: member.signing_bytes(),
            })
            .collect()
    }

    pub fn attach_signatures(
        mut self,
        signatures: Vec<Vec<u8>>,
    ) -> Result<SignedTransaction, ProtocolError> {
        if signatures.len() != self.members.len() {
            return Err(ProtocolError::IncompleteTransaction);
        }
        for (member, signature) in self.members.iter_mut().zip(signatures) {
            if signature.is_empty() || signature.len() > MAX_SIGNATURE_BYTES {
                return Err(ProtocolError::MalformedEnvelope);
            }
            member.signature = signature;
        }
        Ok(SignedTransaction {
            manifest: self.manifest,
            members: self.members,
        })
    }
}

impl SignedTransaction {
    pub fn prepare(
        header: TransactionHeader,
        drafts: Vec<MutationDraft>,
        expires_at: u64,
    ) -> Result<PreparedTransaction, ProtocolError> {
        PreparedTransaction::prepare(header, drafts, expires_at)
    }

    pub fn signed_digest(&self) -> String {
        let mut members = self.members.clone();
        members.sort_by_key(|member| member.transaction_member_index);
        canonical_sha256(&json!({ "manifest": self.manifest, "members": members }))
    }

    pub fn validate(
        &self,
        now: u64,
        negotiated: &NegotiatedCapabilities,
    ) -> Result<(), ProtocolError> {
        if self.manifest.protocol_version != negotiated.protocol_version {
            return Err(ProtocolError::UnsupportedProtocol);
        }
        if self.manifest.expires_at < now {
            return Err(ProtocolError::TransactionExpired);
        }
        if self.manifest.member_count == 0
            || self.manifest.member_count > negotiated.max_transaction_members
            || self.manifest.byte_total > negotiated.max_transaction_bytes
        {
            return Err(ProtocolError::TransactionLimitExceeded);
        }
        if self.members.len() != self.manifest.member_count as usize
            || self.manifest.ordered_member_digests.len() != self.manifest.member_count as usize
        {
            return Err(ProtocolError::IncompleteTransaction);
        }
        if !is_uuid(&self.manifest.library_id)
            || !is_uuid(&self.manifest.transaction_id)
            || !is_uuid(&self.manifest.device_id)
            || self.manifest.device_transaction_counter == 0
            || self.manifest.authority_generation == 0
            || self.manifest.key_epoch == 0
        {
            return Err(ProtocolError::MalformedEnvelope);
        }

        let aggregate_digest = self.manifest.digest();
        let mut ordered = self.members.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|member| member.transaction_member_index);
        let mut mutation_ids = BTreeSet::new();
        let mut record_ids = BTreeSet::new();
        let mut byte_total = 0_u64;
        for (index, member) in ordered.into_iter().enumerate() {
            if member.transaction_member_index != index as u32 {
                return Err(ProtocolError::IncompleteTransaction);
            }
            if member.transaction_member_count != self.manifest.member_count
                || member.transaction_commit_marker != (index + 1 == self.members.len())
                || member.transaction_manifest_digest != aggregate_digest
                || member.protocol_version != self.manifest.protocol_version
                || member.library_id != self.manifest.library_id
                || member.transaction_id != self.manifest.transaction_id
                || member.device_id != self.manifest.device_id
                || member.device_transaction_counter != self.manifest.device_transaction_counter
                || member.authority_generation != self.manifest.authority_generation
                || member.purge_generation != self.manifest.purge_generation
                || member.key_epoch != self.manifest.key_epoch
            {
                return Err(ProtocolError::TransactionManifestMismatch);
            }
            if self.manifest.ordered_member_digests[index] != member.member_digest() {
                return Err(ProtocolError::AggregateDigestMismatch);
            }
            if !mutation_ids.insert(member.mutation_id.as_str())
                || !record_ids.insert(member.record_id.as_str())
            {
                return Err(ProtocolError::DuplicateTransactionMember);
            }
            if !is_uuid(&member.mutation_id)
                || !is_uuid_v7(&member.record_id)
                || !is_uuid(&member.version_id)
                || member
                    .base_head_version_id
                    .as_deref()
                    .is_some_and(|version| !is_uuid(version))
                || member.record_kind.trim().is_empty()
                || member.signature.is_empty()
                || member.signature.len() > MAX_SIGNATURE_BYTES
                || member.ciphertext.is_empty()
                || member.ciphertext_hash != sha256_bytes(&member.ciphertext)
            {
                return Err(ProtocolError::MalformedEnvelope);
            }
            if !member.operation.validates_revision_contract(
                member.base_head_revision,
                member.base_head_version_id.as_deref(),
                member.proposed_revision,
            ) {
                return Err(ProtocolError::MalformedEnvelope);
            }
            match negotiated.access_for(&member.record_kind, member.record_schema_version) {
                RecordAccess::ReadWrite => {}
                RecordAccess::ReadOnly => {
                    return Err(ProtocolError::RecordKindReadOnly {
                        kind: member.record_kind.clone(),
                        schema_version: member.record_schema_version,
                    })
                }
                RecordAccess::Reject => {
                    return Err(ProtocolError::RecordKindUnsupported {
                        kind: member.record_kind.clone(),
                        schema_version: member.record_schema_version,
                    })
                }
            }
            byte_total = byte_total
                .checked_add(member.ciphertext.len() as u64)
                .ok_or(ProtocolError::TransactionLimitExceeded)?;
        }
        if byte_total != self.manifest.byte_total {
            return Err(ProtocolError::TransactionManifestMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedHead {
    pub revision: u64,
    pub version_id: String,
    /// Opaque sequencing commitment. A decrypting replica separately verifies
    /// and stores the portable record's canonical content hash.
    pub ciphertext_hash: String,
    pub authority_generation: u64,
    pub acceptance_checkpoint: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadAdvance {
    pub record_id: String,
    pub record_kind: String,
    pub record_schema_version: u32,
    pub base_revision: u64,
    pub base_version_id: Option<String>,
    pub revision: u64,
    pub version_id: String,
    pub ciphertext_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadConflict {
    pub record_id: String,
    pub proposed_version_id: String,
    pub accepted_head: Option<AcceptedHead>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReceiptDisposition {
    Accepted { advances: Vec<HeadAdvance> },
    Conflict { conflicts: Vec<HeadConflict> },
    Rejected { code: TerminalRejection },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalRejection {
    Expired,
    DeviceRevoked,
    AuthorityGenerationChanged,
    PurgeGenerationChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionReceipt {
    pub library_id: String,
    pub transaction_id: String,
    pub transaction_digest: String,
    pub mutation_ids: Vec<String>,
    pub device_id: String,
    pub device_transaction_counter: u64,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub high_water_cursor: u64,
    pub disposition: ReceiptDisposition,
}

/// One accepted, immutable authority-log entry. It carries only authenticated
/// envelope metadata and ciphertext; plaintext portable records never enter the
/// sequencing layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedChange {
    pub sequence: u64,
    pub transaction_digest: String,
    pub transaction: SignedTransaction,
    pub receipt: TransactionReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePage {
    pub requested_cursor: u64,
    pub next_cursor: u64,
    pub high_water_cursor: u64,
    pub has_more: bool,
    pub changes: Vec<AcceptedChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapRecord {
    pub record_id: String,
    pub accepted_head: AcceptedHead,
    pub mutation: MutationEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapSnapshot {
    pub contract_version: String,
    pub library_id: String,
    pub authority_generation: u64,
    pub purge_generation: u64,
    pub key_epoch: u64,
    pub high_water_cursor: u64,
    pub records: Vec<BootstrapRecord>,
    pub checkpoint_digest: String,
}

impl BootstrapSnapshot {
    pub fn computed_checkpoint_digest(&self) -> String {
        canonical_sha256(&json!({
            "contract_version": self.contract_version,
            "library_id": self.library_id,
            "authority_generation": self.authority_generation,
            "purge_generation": self.purge_generation,
            "key_epoch": self.key_epoch,
            "high_water_cursor": self.high_water_cursor,
            "records": self.records,
        }))
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.contract_version != BOOTSTRAP_SNAPSHOT_VERSION
            || !is_uuid(&self.library_id)
            || self.authority_generation == 0
            || self.key_epoch == 0
            || self.checkpoint_digest != self.computed_checkpoint_digest()
        {
            return Err(ProtocolError::BootstrapSnapshotInvalid);
        }
        let mut previous_record_id: Option<&str> = None;
        let mut mutation_ids = BTreeSet::new();
        for record in &self.records {
            if previous_record_id.is_some_and(|previous| previous >= record.record_id.as_str())
                || !is_uuid_v7(&record.record_id)
                || record.record_id != record.mutation.record_id
                || validate_bootstrap_head(&record.record_id, &record.accepted_head).is_err()
                || record.accepted_head.revision != record.mutation.proposed_revision
                || record.accepted_head.version_id != record.mutation.version_id
                || record.accepted_head.ciphertext_hash != record.mutation.ciphertext_hash
                || record.accepted_head.authority_generation != record.mutation.authority_generation
                || record.accepted_head.authority_generation > self.authority_generation
                || record.accepted_head.acceptance_checkpoint == 0
                || record.accepted_head.acceptance_checkpoint > self.high_water_cursor
                || record.mutation.protocol_version != SYNC_PROTOCOL_VERSION
                || record.mutation.library_id != self.library_id
                || !is_uuid(&record.mutation.mutation_id)
                || !mutation_ids.insert(record.mutation.mutation_id.as_str())
                || !is_uuid(&record.mutation.transaction_id)
                || record.mutation.transaction_member_count == 0
                || record.mutation.transaction_member_count > DEFAULT_MAX_TRANSACTION_MEMBERS
                || record.mutation.transaction_member_index
                    >= record.mutation.transaction_member_count
                || record.mutation.transaction_commit_marker
                    != (record.mutation.transaction_member_index + 1
                        == record.mutation.transaction_member_count)
                || !is_sha256(&record.mutation.transaction_manifest_digest)
                || !is_uuid(&record.mutation.device_id)
                || record.mutation.device_transaction_counter == 0
                || record.mutation.authority_generation == 0
                || record.mutation.authority_generation > self.authority_generation
                || record.mutation.purge_generation > self.purge_generation
                || record.mutation.key_epoch == 0
                || record.mutation.key_epoch > self.key_epoch
                || record.mutation.record_kind.trim().is_empty()
                || record.mutation.record_schema_version == 0
                || !is_uuid(&record.mutation.version_id)
                || record.mutation.signature.is_empty()
                || record.mutation.signature.len() > MAX_SIGNATURE_BYTES
                || record.mutation.ciphertext.is_empty()
                || record.mutation.ciphertext.len() as u64 > DEFAULT_MAX_TRANSACTION_BYTES
                || record.mutation.ciphertext_hash != sha256_bytes(&record.mutation.ciphertext)
                || !record.mutation.operation.validates_revision_contract(
                    record.mutation.base_head_revision,
                    record.mutation.base_head_version_id.as_deref(),
                    record.mutation.proposed_revision,
                )
                || record
                    .mutation
                    .base_head_version_id
                    .as_deref()
                    .is_some_and(|version_id| !is_uuid(version_id))
            {
                return Err(ProtocolError::BootstrapSnapshotInvalid);
            }
            previous_record_id = Some(&record.record_id);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginOutcome {
    Prepared,
    PendingReplay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Terminal(TransactionReceipt),
    Replay(TransactionReceipt),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceRegistration {
    capabilities: ProtocolCapabilities,
    revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct DeviceCounterState {
    last_reserved: u64,
    bindings: BTreeMap<u64, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransactionRecord {
    transaction: SignedTransaction,
    signed_digest: String,
    receipt: Option<TransactionReceipt>,
}

/// Serializable durable sequencing state. `begin_transaction` persists all ID
/// and counter reservations; `finish_transaction` performs one terminal state
/// transition. Persisting between those calls is sufficient for exact replay
/// after a crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityState {
    library_id: String,
    authority_generation: u64,
    purge_generation: u64,
    current_key_epoch: u64,
    high_water_cursor: u64,
    capabilities: ProtocolCapabilities,
    devices: BTreeMap<String, DeviceRegistration>,
    heads: BTreeMap<String, AcceptedHead>,
    accepted_changes: BTreeMap<u64, AcceptedChange>,
    mutation_bindings: BTreeMap<String, String>,
    transactions: BTreeMap<String, TransactionRecord>,
    counters: BTreeMap<String, DeviceCounterState>,
}

impl AuthorityState {
    pub fn new(
        library_id: String,
        authority_generation: u64,
        purge_generation: u64,
        current_key_epoch: u64,
        capabilities: ProtocolCapabilities,
    ) -> Result<Self, ProtocolError> {
        if !is_uuid(&library_id) || authority_generation == 0 || current_key_epoch == 0 {
            return Err(ProtocolError::MalformedEnvelope);
        }
        capabilities.validate()?;
        Ok(Self {
            library_id,
            authority_generation,
            purge_generation,
            current_key_epoch,
            high_water_cursor: 0,
            capabilities,
            devices: BTreeMap::new(),
            heads: BTreeMap::new(),
            accepted_changes: BTreeMap::new(),
            mutation_bindings: BTreeMap::new(),
            transactions: BTreeMap::new(),
            counters: BTreeMap::new(),
        })
    }

    pub fn register_device(
        &mut self,
        device_id: String,
        capabilities: ProtocolCapabilities,
    ) -> Result<(), ProtocolError> {
        if !is_uuid(&device_id) {
            return Err(ProtocolError::MalformedEnvelope);
        }
        capabilities.validate()?;
        match self.devices.get(&device_id) {
            Some(existing) if existing.capabilities == capabilities && !existing.revoked => Ok(()),
            Some(_) => Err(ProtocolError::DeviceRegistrationMismatch),
            None => {
                self.devices.insert(
                    device_id,
                    DeviceRegistration {
                        capabilities,
                        revoked: false,
                    },
                );
                Ok(())
            }
        }
    }

    pub fn revoke_device(&mut self, device_id: &str) -> Result<(), ProtocolError> {
        let device = self
            .devices
            .get_mut(device_id)
            .ok_or(ProtocolError::DeviceUnknown)?;
        device.revoked = true;
        Ok(())
    }

    pub fn accepted_head(&self, record_id: &str) -> Option<&AcceptedHead> {
        self.heads.get(record_id)
    }

    pub fn high_water_cursor(&self) -> u64 {
        self.high_water_cursor
    }

    pub fn library_id(&self) -> &str {
        &self.library_id
    }

    pub fn authority_generation(&self) -> u64 {
        self.authority_generation
    }

    pub fn purge_generation(&self) -> u64 {
        self.purge_generation
    }

    pub fn current_key_epoch(&self) -> u64 {
        self.current_key_epoch
    }

    pub fn capabilities(&self) -> &ProtocolCapabilities {
        &self.capabilities
    }

    /// Return a bounded, contiguous page after the caller's durable cursor.
    /// A cursor beyond the authority high-water mark is rejected instead of
    /// being interpreted as an empty page, which prevents silent rollback or
    /// cross-authority cursor reuse.
    pub fn changes_after(&self, cursor: u64, limit: u32) -> Result<ChangePage, ProtocolError> {
        if limit == 0 || limit > MAX_PULL_PAGE_CHANGES {
            return Err(ProtocolError::InvalidPullLimit {
                maximum: MAX_PULL_PAGE_CHANGES,
                provided: limit,
            });
        }
        if cursor > self.high_water_cursor {
            return Err(ProtocolError::CursorAhead {
                high_water: self.high_water_cursor,
                provided: cursor,
            });
        }

        let available = self.high_water_cursor - cursor;
        let take = available.min(u64::from(limit));
        let mut changes = Vec::with_capacity(take as usize);
        if take > 0 {
            let first = cursor.checked_add(1).ok_or(ProtocolError::CursorOverflow)?;
            let last = cursor
                .checked_add(take)
                .ok_or(ProtocolError::CursorOverflow)?;
            for sequence in first..=last {
                changes.push(
                    self.accepted_changes
                        .get(&sequence)
                        .cloned()
                        .ok_or_else(|| {
                            ProtocolError::CheckpointCorrupt(format!(
                                "accepted change sequence {sequence} is missing"
                            ))
                        })?,
                );
            }
        }
        let next_cursor = cursor + take;
        Ok(ChangePage {
            requested_cursor: cursor,
            next_cursor,
            high_water_cursor: self.high_water_cursor,
            has_more: next_cursor < self.high_water_cursor,
            changes,
        })
    }

    /// Freeze the current accepted heads and their opaque ciphertext envelopes
    /// into a deterministic bootstrap value. Later accepted writes produce a
    /// new snapshot and cannot mutate a snapshot already handed to a caller.
    pub fn bootstrap_snapshot(&self) -> Result<BootstrapSnapshot, ProtocolError> {
        let mut records = Vec::with_capacity(self.heads.len());
        for (record_id, head) in &self.heads {
            let change = self
                .accepted_changes
                .get(&head.acceptance_checkpoint)
                .ok_or_else(|| {
                    ProtocolError::CheckpointCorrupt(format!(
                        "head {record_id} references missing accepted change {}",
                        head.acceptance_checkpoint
                    ))
                })?;
            let mutation = change
                .transaction
                .members
                .iter()
                .find(|member| {
                    member.record_id == *record_id && member.version_id == head.version_id
                })
                .cloned()
                .ok_or_else(|| {
                    ProtocolError::CheckpointCorrupt(format!(
                        "head {record_id} has no matching accepted ciphertext"
                    ))
                })?;
            records.push(BootstrapRecord {
                record_id: record_id.clone(),
                accepted_head: head.clone(),
                mutation,
            });
        }
        let mut snapshot = BootstrapSnapshot {
            contract_version: BOOTSTRAP_SNAPSHOT_VERSION.to_string(),
            library_id: self.library_id.clone(),
            authority_generation: self.authority_generation,
            purge_generation: self.purge_generation,
            key_epoch: self.current_key_epoch,
            high_water_cursor: self.high_water_cursor,
            records,
            checkpoint_digest: String::new(),
        };
        snapshot.checkpoint_digest = snapshot.computed_checkpoint_digest();
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn begin_transaction(
        &mut self,
        mut transaction: SignedTransaction,
        now: u64,
    ) -> Result<BeginOutcome, ProtocolError> {
        let transaction_id = transaction.manifest.transaction_id.clone();
        let signed_digest = transaction.signed_digest();
        if let Some(existing) = self.transactions.get(&transaction_id) {
            if existing.signed_digest != signed_digest {
                return Err(ProtocolError::TransactionIdReuse);
            }
            return Ok(BeginOutcome::PendingReplay);
        }

        if transaction.manifest.library_id != self.library_id {
            return Err(ProtocolError::WrongLibrary);
        }
        let registration = self
            .devices
            .get(&transaction.manifest.device_id)
            .ok_or(ProtocolError::DeviceUnknown)?;
        if registration.revoked {
            return Err(ProtocolError::DeviceRevoked);
        }
        let negotiated = negotiate_capabilities(&self.capabilities, &registration.capabilities)?;
        transaction.validate(now, &negotiated)?;
        self.validate_generation_floors(&transaction.manifest)?;
        // Normalize transport arrival order before persisting or producing a
        // receipt. The signed transaction digest is already order-independent.
        transaction
            .members
            .sort_by_key(|member| member.transaction_member_index);

        for member in &transaction.members {
            if let Some(bound_digest) = self.mutation_bindings.get(&member.mutation_id) {
                if bound_digest != &member.signed_digest() {
                    return Err(ProtocolError::MutationIdReuse);
                }
                return Err(ProtocolError::MutationIdReuse);
            }
        }

        if self.transactions.values().any(|record| {
            record.receipt.is_none()
                && record.transaction.manifest.device_id == transaction.manifest.device_id
        }) {
            return Err(ProtocolError::PriorTransactionPending);
        }

        let counter = self
            .counters
            .entry(transaction.manifest.device_id.clone())
            .or_default();
        let requested = transaction.manifest.device_transaction_counter;
        let expected = counter
            .last_reserved
            .checked_add(1)
            .ok_or(ProtocolError::CounterGap {
                expected: u64::MAX,
                provided: requested,
            })?;
        if requested != expected {
            if let Some(bound_digest) = counter.bindings.get(&requested) {
                if bound_digest != &signed_digest {
                    return Err(ProtocolError::CounterReuse);
                }
                return Err(ProtocolError::CounterReuse);
            }
            return Err(ProtocolError::CounterGap {
                expected,
                provided: requested,
            });
        }

        counter.last_reserved = requested;
        counter.bindings.insert(requested, signed_digest.clone());
        for member in &transaction.members {
            self.mutation_bindings
                .insert(member.mutation_id.clone(), member.signed_digest());
        }
        self.transactions.insert(
            transaction_id,
            TransactionRecord {
                transaction,
                signed_digest,
                receipt: None,
            },
        );
        Ok(BeginOutcome::Prepared)
    }

    pub fn finish_transaction(
        &mut self,
        transaction_id: &str,
        now: u64,
    ) -> Result<SubmitOutcome, ProtocolError> {
        let record = self
            .transactions
            .get(transaction_id)
            .ok_or(ProtocolError::TransactionUnknown)?;
        if let Some(receipt) = &record.receipt {
            return Ok(SubmitOutcome::Replay(receipt.clone()));
        }
        let transaction = record.transaction.clone();
        let signed_digest = record.signed_digest.clone();

        let device_is_revoked = self
            .devices
            .get(&transaction.manifest.device_id)
            .is_none_or(|device| device.revoked);
        let disposition = if device_is_revoked {
            ReceiptDisposition::Rejected {
                code: TerminalRejection::DeviceRevoked,
            }
        } else if transaction.manifest.expires_at < now {
            ReceiptDisposition::Rejected {
                code: TerminalRejection::Expired,
            }
        } else if transaction.manifest.authority_generation != self.authority_generation {
            ReceiptDisposition::Rejected {
                code: TerminalRejection::AuthorityGenerationChanged,
            }
        } else if transaction.manifest.purge_generation != self.purge_generation {
            ReceiptDisposition::Rejected {
                code: TerminalRejection::PurgeGenerationChanged,
            }
        } else {
            let mut conflicts = Vec::new();
            for member in &transaction.members {
                let current = self.heads.get(&member.record_id);
                let matches = match current {
                    None => member.base_head_revision == 0 && member.base_head_version_id.is_none(),
                    Some(head) => {
                        member.base_head_revision == head.revision
                            && member.base_head_version_id.as_deref()
                                == Some(head.version_id.as_str())
                    }
                };
                if !matches {
                    conflicts.push(HeadConflict {
                        record_id: member.record_id.clone(),
                        proposed_version_id: member.version_id.clone(),
                        accepted_head: current.cloned(),
                    });
                }
            }
            if conflicts.is_empty() {
                self.high_water_cursor = self
                    .high_water_cursor
                    .checked_add(1)
                    .ok_or(ProtocolError::CursorOverflow)?;
                let advances = transaction
                    .members
                    .iter()
                    .map(|member| HeadAdvance {
                        record_id: member.record_id.clone(),
                        record_kind: member.record_kind.clone(),
                        record_schema_version: member.record_schema_version,
                        base_revision: member.base_head_revision,
                        base_version_id: member.base_head_version_id.clone(),
                        revision: member.proposed_revision,
                        version_id: member.version_id.clone(),
                        ciphertext_hash: member.ciphertext_hash.clone(),
                    })
                    .collect::<Vec<_>>();
                for advance in &advances {
                    self.heads.insert(
                        advance.record_id.clone(),
                        AcceptedHead {
                            revision: advance.revision,
                            version_id: advance.version_id.clone(),
                            ciphertext_hash: advance.ciphertext_hash.clone(),
                            authority_generation: self.authority_generation,
                            acceptance_checkpoint: self.high_water_cursor,
                        },
                    );
                }
                ReceiptDisposition::Accepted { advances }
            } else {
                ReceiptDisposition::Conflict { conflicts }
            }
        };

        let is_accepted = matches!(&disposition, ReceiptDisposition::Accepted { .. });
        let receipt = TransactionReceipt {
            library_id: self.library_id.clone(),
            transaction_id: transaction.manifest.transaction_id.clone(),
            transaction_digest: signed_digest.clone(),
            mutation_ids: transaction
                .members
                .iter()
                .map(|member| member.mutation_id.clone())
                .collect(),
            device_id: transaction.manifest.device_id.clone(),
            device_transaction_counter: transaction.manifest.device_transaction_counter,
            authority_generation: self.authority_generation,
            purge_generation: self.purge_generation,
            high_water_cursor: self.high_water_cursor,
            disposition,
        };
        if is_accepted {
            let change = AcceptedChange {
                sequence: self.high_water_cursor,
                transaction_digest: signed_digest,
                transaction: transaction.clone(),
                receipt: receipt.clone(),
            };
            assert!(
                self.accepted_changes
                    .insert(self.high_water_cursor, change)
                    .is_none(),
                "accepted change sequence is append-only"
            );
        }
        self.transactions
            .get_mut(transaction_id)
            .expect("prepared transaction remains present")
            .receipt = Some(receipt.clone());
        Ok(SubmitOutcome::Terminal(receipt))
    }

    pub fn submit_transaction(
        &mut self,
        transaction: SignedTransaction,
        now: u64,
    ) -> Result<SubmitOutcome, ProtocolError> {
        let transaction_id = transaction.manifest.transaction_id.clone();
        let begin = self.begin_transaction(transaction, now)?;
        let finished = self.finish_transaction(&transaction_id, now)?;
        match (begin, finished) {
            (BeginOutcome::PendingReplay, SubmitOutcome::Terminal(receipt))
            | (BeginOutcome::PendingReplay, SubmitOutcome::Replay(receipt)) => {
                Ok(SubmitOutcome::Replay(receipt))
            }
            (_, outcome) => Ok(outcome),
        }
    }

    pub fn checkpoint_json(&self) -> Result<String, ProtocolError> {
        serde_json::to_string(self)
            .map_err(|error| ProtocolError::CheckpointCorrupt(error.to_string()))
    }

    pub fn from_checkpoint_json(checkpoint: &str) -> Result<Self, ProtocolError> {
        let state: Self = serde_json::from_str(checkpoint)
            .map_err(|error| ProtocolError::CheckpointCorrupt(error.to_string()))?;
        state.validate_checkpoint()?;
        Ok(state)
    }

    fn validate_generation_floors(
        &self,
        manifest: &TransactionManifest,
    ) -> Result<(), ProtocolError> {
        if manifest.authority_generation < self.authority_generation {
            return Err(ProtocolError::AuthorityGenerationStale {
                minimum: self.authority_generation,
                provided: manifest.authority_generation,
            });
        }
        if manifest.authority_generation > self.authority_generation {
            return Err(ProtocolError::AuthorityGenerationAhead {
                current: self.authority_generation,
                provided: manifest.authority_generation,
            });
        }
        if manifest.purge_generation < self.purge_generation {
            return Err(ProtocolError::PurgeGenerationStale {
                minimum: self.purge_generation,
                provided: manifest.purge_generation,
            });
        }
        if manifest.purge_generation > self.purge_generation {
            return Err(ProtocolError::PurgeGenerationAhead {
                current: self.purge_generation,
                provided: manifest.purge_generation,
            });
        }
        if manifest.key_epoch < self.current_key_epoch {
            return Err(ProtocolError::KeyEpochStale {
                minimum: self.current_key_epoch,
                provided: manifest.key_epoch,
            });
        }
        if manifest.key_epoch > self.current_key_epoch {
            return Err(ProtocolError::KeyEpochAhead {
                current: self.current_key_epoch,
                provided: manifest.key_epoch,
            });
        }
        Ok(())
    }

    fn validate_checkpoint(&self) -> Result<(), ProtocolError> {
        if !is_uuid(&self.library_id)
            || self.authority_generation == 0
            || self.current_key_epoch == 0
        {
            return Err(ProtocolError::CheckpointCorrupt(
                "invalid authority identity or generation".to_string(),
            ));
        }
        self.capabilities.validate()?;
        for (transaction_id, record) in &self.transactions {
            let registration = self
                .devices
                .get(&record.transaction.manifest.device_id)
                .ok_or_else(|| {
                    ProtocolError::CheckpointCorrupt(
                        "transaction references an unknown device".to_string(),
                    )
                })?;
            let negotiated = negotiate_capabilities(&self.capabilities, &registration.capabilities)
                .map_err(|error| ProtocolError::CheckpointCorrupt(error.to_string()))?;
            record
                .transaction
                .validate(0, &negotiated)
                .map_err(|error| ProtocolError::CheckpointCorrupt(error.to_string()))?;
            if transaction_id != &record.transaction.manifest.transaction_id
                || record.signed_digest != record.transaction.signed_digest()
            {
                return Err(ProtocolError::CheckpointCorrupt(
                    "transaction digest binding changed".to_string(),
                ));
            }
            if let Some(receipt) = &record.receipt {
                if receipt.transaction_id != *transaction_id
                    || receipt.transaction_digest != record.signed_digest
                {
                    return Err(ProtocolError::CheckpointCorrupt(
                        "terminal receipt binding changed".to_string(),
                    ));
                }
            }
            for member in &record.transaction.members {
                if self.mutation_bindings.get(&member.mutation_id) != Some(&member.signed_digest())
                {
                    return Err(ProtocolError::CheckpointCorrupt(
                        "mutation digest binding changed".to_string(),
                    ));
                }
            }
            let counter = self
                .counters
                .get(&record.transaction.manifest.device_id)
                .and_then(|state| {
                    state
                        .bindings
                        .get(&record.transaction.manifest.device_transaction_counter)
                });
            if counter != Some(&record.signed_digest) {
                return Err(ProtocolError::CheckpointCorrupt(
                    "device counter binding changed".to_string(),
                ));
            }
        }
        if self.accepted_changes.len() as u64 != self.high_water_cursor {
            return Err(ProtocolError::CheckpointCorrupt(
                "accepted change log is not contiguous through its high-water cursor".to_string(),
            ));
        }
        let mut reconstructed_heads = BTreeMap::new();
        for sequence in 1..=self.high_water_cursor {
            let change = self.accepted_changes.get(&sequence).ok_or_else(|| {
                ProtocolError::CheckpointCorrupt(format!(
                    "accepted change sequence {sequence} is missing"
                ))
            })?;
            let stored_transaction = self
                .transactions
                .get(&change.receipt.transaction_id)
                .ok_or_else(|| {
                    ProtocolError::CheckpointCorrupt(format!(
                        "accepted change sequence {sequence} has no transaction record"
                    ))
                })?;
            if change.sequence != sequence
                || change.transaction_digest != change.transaction.signed_digest()
                || change.receipt.transaction_digest != change.transaction_digest
                || change.receipt.transaction_id != change.transaction.manifest.transaction_id
                || change.receipt.high_water_cursor != sequence
                || stored_transaction.transaction != change.transaction
                || stored_transaction.receipt.as_ref() != Some(&change.receipt)
            {
                return Err(ProtocolError::CheckpointCorrupt(format!(
                    "accepted change sequence {sequence} has inconsistent bindings"
                )));
            }
            let ReceiptDisposition::Accepted { advances } = &change.receipt.disposition else {
                return Err(ProtocolError::CheckpointCorrupt(format!(
                    "non-accepted transaction appears at change sequence {sequence}"
                )));
            };
            if advances.len() != change.transaction.members.len()
                || change.transaction.members.iter().any(|member| {
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
                return Err(ProtocolError::CheckpointCorrupt(format!(
                    "accepted change sequence {sequence} does not match its ciphertext members"
                )));
            }
            for advance in advances {
                reconstructed_heads.insert(
                    advance.record_id.clone(),
                    AcceptedHead {
                        revision: advance.revision,
                        version_id: advance.version_id.clone(),
                        ciphertext_hash: advance.ciphertext_hash.clone(),
                        authority_generation: change.receipt.authority_generation,
                        acceptance_checkpoint: sequence,
                    },
                );
            }
        }
        if reconstructed_heads != self.heads {
            return Err(ProtocolError::CheckpointCorrupt(
                "accepted heads do not match the immutable change log".to_string(),
            ));
        }
        for counter in self.counters.values() {
            if counter.bindings.keys().next_back().copied().unwrap_or(0) != counter.last_reserved {
                return Err(ProtocolError::CheckpointCorrupt(
                    "device counter high-water mark changed".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalBranchStatus {
    Pending,
    Accepted,
    Conflict,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBranch {
    pub branch_id: String,
    pub mutation_id: String,
    pub record_id: String,
    pub base_revision: u64,
    pub base_version_id: Option<String>,
    pub proposed_revision: u64,
    pub version_id: String,
    pub content_hash: String,
    pub status: LocalBranchStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaAcceptedHead {
    pub revision: u64,
    pub version_id: String,
    pub content_hash: String,
    pub authority_generation: u64,
    pub acceptance_checkpoint: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedRecordAdvance {
    /// Mutation and ciphertext identifiers from the authenticated envelope
    /// that produced this decoded canonical record.
    pub source_mutation_id: String,
    pub source_ciphertext_hash: String,
    pub record_id: String,
    pub record_kind: String,
    pub record_schema_version: u32,
    pub base_revision: u64,
    pub base_version_id: Option<String>,
    pub revision: u64,
    pub version_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedChange {
    /// The complete authority-accepted transaction whose ciphertext members
    /// were authenticated and decoded. Application is all-or-nothing: every
    /// accepted member must have exactly one decoded record and vice versa.
    pub accepted_change: AcceptedChange,
    pub records: Vec<DecodedRecordAdvance>,
}

/// Opaque proof that the transport verifier authenticated an authority-issued
/// quarantine receipt. It is intentionally non-serializable, with a
/// crate-private constructor and private fields, so untrusted API callers
/// cannot mint a value that advances the applied cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedQuarantineReceipt {
    library_id: String,
    sequence: u64,
    transaction_digest: String,
    authority_generation: u64,
    purge_generation: u64,
    evidence_digest: String,
    reason_code: String,
}

impl AuthenticatedQuarantineReceipt {
    /// This constructor is the verifier boundary. Callers must invoke it only
    /// after authenticating the exact authority receipt bytes, including every
    /// claim passed here.
    #[allow(dead_code)]
    pub(crate) fn from_verified_authority_receipt(
        library_id: String,
        sequence: u64,
        transaction_digest: String,
        authority_generation: u64,
        purge_generation: u64,
        evidence_digest: String,
        reason_code: String,
    ) -> Result<Self, ProtocolError> {
        if !is_uuid(&library_id)
            || sequence == 0
            || !is_sha256(&transaction_digest)
            || !is_sha256(&evidence_digest)
            || authority_generation == 0
            || reason_code.trim().is_empty()
        {
            return Err(ProtocolError::MalformedEnvelope);
        }
        Ok(Self {
            library_id,
            sequence,
            transaction_digest,
            authority_generation,
            purge_generation,
            evidence_digest,
            reason_code,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantinedChange {
    pub sequence: u64,
    pub transaction_digest: String,
    pub evidence_digest: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaState {
    library_id: String,
    authority_generation: u64,
    purge_generation: u64,
    downloaded_cursor: u64,
    applied_cursor: u64,
    accepted_heads: BTreeMap<String, ReplicaAcceptedHead>,
    branches: BTreeMap<String, LocalBranch>,
    inbox: BTreeMap<u64, String>,
    applied_bindings: BTreeMap<u64, String>,
    quarantine: BTreeMap<u64, QuarantinedChange>,
}

impl ReplicaState {
    pub fn new(
        library_id: String,
        authority_generation: u64,
        purge_generation: u64,
    ) -> Result<Self, ProtocolError> {
        if !is_uuid(&library_id) || authority_generation == 0 {
            return Err(ProtocolError::MalformedEnvelope);
        }
        Ok(Self {
            library_id,
            authority_generation,
            purge_generation,
            downloaded_cursor: 0,
            applied_cursor: 0,
            accepted_heads: BTreeMap::new(),
            branches: BTreeMap::new(),
            inbox: BTreeMap::new(),
            applied_bindings: BTreeMap::new(),
            quarantine: BTreeMap::new(),
        })
    }

    pub fn accepted_head(&self, record_id: &str) -> Option<&ReplicaAcceptedHead> {
        self.accepted_heads.get(record_id)
    }

    pub fn branch(&self, branch_id: &str) -> Option<&LocalBranch> {
        self.branches.get(branch_id)
    }

    pub fn downloaded_cursor(&self) -> u64 {
        self.downloaded_cursor
    }

    pub fn applied_cursor(&self) -> u64 {
        self.applied_cursor
    }

    pub fn quarantined_change(&self, sequence: u64) -> Option<&QuarantinedChange> {
        self.quarantine.get(&sequence)
    }

    pub fn seed_accepted_head(
        &mut self,
        record_id: String,
        head: ReplicaAcceptedHead,
    ) -> Result<(), ProtocolError> {
        validate_replica_head(&record_id, &head)?;
        self.accepted_heads.insert(record_id, head);
        Ok(())
    }

    pub fn stage_local_branch(
        &mut self,
        branch_id: String,
        mutation_id: String,
        record_id: String,
        version_id: String,
        content_hash: String,
    ) -> Result<LocalBranch, ProtocolError> {
        if !is_uuid(&branch_id)
            || !is_uuid(&mutation_id)
            || !is_uuid_v7(&record_id)
            || !is_uuid(&version_id)
            || !is_sha256(&content_hash)
            || self.branches.contains_key(&branch_id)
        {
            return Err(ProtocolError::MalformedEnvelope);
        }
        let (base_revision, base_version_id) = self
            .accepted_heads
            .get(&record_id)
            .map(|head| (head.revision, Some(head.version_id.clone())))
            .unwrap_or((0, None));
        let branch = LocalBranch {
            branch_id: branch_id.clone(),
            mutation_id,
            record_id,
            base_revision,
            base_version_id,
            proposed_revision: base_revision
                .checked_add(1)
                .ok_or(ProtocolError::MalformedEnvelope)?,
            version_id,
            content_hash,
            status: LocalBranchStatus::Pending,
        };
        self.branches.insert(branch_id, branch.clone());
        Ok(branch)
    }

    pub fn observe_receipt(&mut self, receipt: &TransactionReceipt) -> Result<(), ProtocolError> {
        if receipt.library_id != self.library_id {
            return Err(ProtocolError::WrongLibrary);
        }
        self.validate_generation(receipt.authority_generation, receipt.purge_generation)?;
        let receipt_mutation_ids = receipt
            .mutation_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if receipt_mutation_ids.is_empty()
            || receipt_mutation_ids.len() != receipt.mutation_ids.len()
            || receipt_mutation_ids
                .iter()
                .any(|mutation_id| !is_uuid(mutation_id))
        {
            return Err(ProtocolError::MalformedEnvelope);
        }
        match &receipt.disposition {
            ReceiptDisposition::Accepted { advances } => {
                for advance in advances {
                    for branch in self.branches.values_mut().filter(|branch| {
                        branch.version_id == advance.version_id
                            && branch.record_id == advance.record_id
                    }) {
                        branch.status = LocalBranchStatus::Accepted;
                        self.accepted_heads.insert(
                            advance.record_id.clone(),
                            ReplicaAcceptedHead {
                                revision: advance.revision,
                                version_id: advance.version_id.clone(),
                                content_hash: branch.content_hash.clone(),
                                authority_generation: receipt.authority_generation,
                                acceptance_checkpoint: receipt.high_water_cursor,
                            },
                        );
                    }
                }
            }
            ReceiptDisposition::Conflict { .. } => {
                // A conflict rejects the complete atomic transaction, not only
                // the member(s) whose base comparison exposed the conflict.
                for branch in self
                    .branches
                    .values_mut()
                    .filter(|branch| receipt_mutation_ids.contains(branch.mutation_id.as_str()))
                {
                    branch.status = LocalBranchStatus::Conflict;
                }
            }
            ReceiptDisposition::Rejected { .. } => {
                for branch in self
                    .branches
                    .values_mut()
                    .filter(|branch| receipt.mutation_ids.contains(&branch.mutation_id))
                {
                    branch.status = LocalBranchStatus::Conflict;
                }
            }
        }
        Ok(())
    }

    /// Durably records authenticated ciphertext. Reordered delivery is allowed;
    /// the downloaded cursor advances only through the highest contiguous run.
    pub fn record_downloaded(
        &mut self,
        sequence: u64,
        transaction_digest: String,
    ) -> Result<(), ProtocolError> {
        if sequence == 0 || !is_sha256(&transaction_digest) {
            return Err(ProtocolError::MalformedEnvelope);
        }
        if sequence <= self.applied_cursor {
            return if self.applied_bindings.get(&sequence) == Some(&transaction_digest) {
                Ok(())
            } else {
                Err(ProtocolError::DownloadedSequenceReuse)
            };
        }
        if let Some(existing) = self.inbox.get(&sequence) {
            return if existing == &transaction_digest {
                Ok(())
            } else {
                Err(ProtocolError::DownloadedSequenceReuse)
            };
        }
        self.inbox.insert(sequence, transaction_digest);
        while self.inbox.contains_key(&(self.downloaded_cursor + 1)) {
            self.downloaded_cursor += 1;
        }
        Ok(())
    }

    pub fn apply_decoded_change(&mut self, change: DecodedChange) -> Result<(), ProtocolError> {
        let accepted_change = &change.accepted_change;
        if accepted_change.receipt.library_id != self.library_id {
            return Err(ProtocolError::WrongLibrary);
        }
        let advances = validate_accepted_change_bindings(accepted_change)?;
        self.validate_next_change(
            accepted_change.sequence,
            &accepted_change.transaction_digest,
            accepted_change.receipt.authority_generation,
            accepted_change.receipt.purge_generation,
        )?;
        if change.records.len() != accepted_change.transaction.members.len() {
            return Err(ProtocolError::IncompleteTransaction);
        }
        let mut record_ids = BTreeSet::new();
        let mut mutation_ids = BTreeSet::new();
        for record in &change.records {
            if !record_ids.insert(record.record_id.as_str())
                || !mutation_ids.insert(record.source_mutation_id.as_str())
                || !is_uuid(&record.source_mutation_id)
                || !is_sha256(&record.source_ciphertext_hash)
                || !is_uuid_v7(&record.record_id)
                || record.record_kind.trim().is_empty()
                || record.record_schema_version == 0
                || !is_uuid(&record.version_id)
                || !is_sha256(&record.content_hash)
                || record
                    .base_revision
                    .checked_add(1)
                    .is_none_or(|revision| revision != record.revision)
                || (record.base_revision == 0 && record.base_version_id.is_some())
                || (record.base_revision > 0 && record.base_version_id.is_none())
            {
                return Err(ProtocolError::MalformedEnvelope);
            }

            let member = accepted_change
                .transaction
                .members
                .iter()
                .find(|member| member.mutation_id == record.source_mutation_id)
                .ok_or(ProtocolError::DecodedTransactionMismatch)?;
            let advance = advances
                .iter()
                .find(|advance| advance.record_id == member.record_id)
                .ok_or(ProtocolError::DecodedTransactionMismatch)?;
            if record.source_ciphertext_hash != member.ciphertext_hash
                || record.record_id != member.record_id
                || record.record_kind != member.record_kind
                || record.record_schema_version != member.record_schema_version
                || record.base_revision != member.base_head_revision
                || record.base_version_id != member.base_head_version_id
                || record.revision != member.proposed_revision
                || record.version_id != member.version_id
                || !advance_matches_member(advance, member)
            {
                return Err(ProtocolError::DecodedTransactionMismatch);
            }
            let current = self.accepted_heads.get(&record.record_id);
            let base_matches = match current {
                None => record.base_revision == 0,
                Some(head) => {
                    record.base_revision == head.revision
                        && record.base_version_id.as_deref() == Some(head.version_id.as_str())
                }
            };
            if !base_matches {
                return Err(ProtocolError::ReplicaBaseMismatch);
            }
        }

        for record in change.records {
            for branch in self
                .branches
                .values_mut()
                .filter(|branch| branch.record_id == record.record_id)
            {
                if branch.version_id == record.version_id {
                    branch.status = LocalBranchStatus::Accepted;
                } else if branch.status == LocalBranchStatus::Pending
                    && branch.base_revision < record.revision
                {
                    branch.status = LocalBranchStatus::Conflict;
                }
            }
            self.accepted_heads.insert(
                record.record_id,
                ReplicaAcceptedHead {
                    revision: record.revision,
                    version_id: record.version_id,
                    content_hash: record.content_hash,
                    authority_generation: accepted_change.receipt.authority_generation,
                    acceptance_checkpoint: accepted_change.sequence,
                },
            );
        }
        self.inbox.remove(&accepted_change.sequence);
        self.applied_bindings.insert(
            accepted_change.sequence,
            accepted_change.transaction_digest.clone(),
        );
        self.applied_cursor = accepted_change.sequence;
        Ok(())
    }

    pub fn quarantine_poison(
        &mut self,
        authorization: AuthenticatedQuarantineReceipt,
    ) -> Result<(), ProtocolError> {
        let sequence = authorization.sequence;
        let transaction_digest = self
            .inbox
            .get(&sequence)
            .ok_or(ProtocolError::DownloadedChangeMissing)?
            .clone();
        self.validate_next_change(
            sequence,
            &transaction_digest,
            authorization.authority_generation,
            authorization.purge_generation,
        )?;
        if authorization.library_id != self.library_id
            || authorization.transaction_digest != transaction_digest
        {
            return Err(ProtocolError::QuarantineReceiptMismatch);
        }
        self.quarantine.insert(
            sequence,
            QuarantinedChange {
                sequence,
                transaction_digest: transaction_digest.clone(),
                evidence_digest: authorization.evidence_digest,
                reason_code: authorization.reason_code,
            },
        );
        self.inbox.remove(&sequence);
        self.applied_bindings.insert(sequence, transaction_digest);
        self.applied_cursor = sequence;
        Ok(())
    }

    fn validate_next_change(
        &self,
        sequence: u64,
        transaction_digest: &str,
        authority_generation: u64,
        purge_generation: u64,
    ) -> Result<(), ProtocolError> {
        let expected = self
            .applied_cursor
            .checked_add(1)
            .ok_or(ProtocolError::CursorOverflow)?;
        if sequence != expected {
            return Err(ProtocolError::CursorGap {
                expected,
                provided: sequence,
            });
        }
        if self.inbox.get(&sequence).map(String::as_str) != Some(transaction_digest) {
            return Err(ProtocolError::DownloadedChangeMissing);
        }
        self.validate_generation(authority_generation, purge_generation)
    }

    fn validate_generation(
        &self,
        authority_generation: u64,
        purge_generation: u64,
    ) -> Result<(), ProtocolError> {
        if authority_generation < self.authority_generation {
            return Err(ProtocolError::AuthorityGenerationStale {
                minimum: self.authority_generation,
                provided: authority_generation,
            });
        }
        if authority_generation > self.authority_generation {
            return Err(ProtocolError::AuthorityGenerationAhead {
                current: self.authority_generation,
                provided: authority_generation,
            });
        }
        if purge_generation < self.purge_generation {
            return Err(ProtocolError::PurgeGenerationStale {
                minimum: self.purge_generation,
                provided: purge_generation,
            });
        }
        if purge_generation > self.purge_generation {
            return Err(ProtocolError::PurgeGenerationAhead {
                current: self.purge_generation,
                provided: purge_generation,
            });
        }
        Ok(())
    }
}

fn validate_accepted_change_bindings(
    change: &AcceptedChange,
) -> Result<&[HeadAdvance], ProtocolError> {
    let transaction = &change.transaction;
    let receipt = &change.receipt;
    if change.sequence == 0
        || transaction.manifest.protocol_version != SYNC_PROTOCOL_VERSION
        || change.transaction_digest != transaction.signed_digest()
        || receipt.library_id != transaction.manifest.library_id
        || receipt.transaction_id != transaction.manifest.transaction_id
        || receipt.transaction_digest != change.transaction_digest
        || receipt.device_id != transaction.manifest.device_id
        || receipt.device_transaction_counter != transaction.manifest.device_transaction_counter
        || receipt.authority_generation != transaction.manifest.authority_generation
        || receipt.purge_generation != transaction.manifest.purge_generation
        || receipt.high_water_cursor != change.sequence
    {
        return Err(ProtocolError::DecodedTransactionMismatch);
    }

    // Re-run the complete envelope/manifest validation without applying the
    // original expiry wall. Accepted changes remain valid pull history after
    // their upload deadline has elapsed.
    let mut record_kinds: BTreeMap<String, NegotiatedRecordCapability> = BTreeMap::new();
    for member in &transaction.members {
        record_kinds
            .entry(member.record_kind.clone())
            .and_modify(|capability| {
                capability.max_read_schema_version = capability
                    .max_read_schema_version
                    .max(member.record_schema_version);
                capability.max_write_schema_version = capability
                    .max_write_schema_version
                    .max(member.record_schema_version);
            })
            .or_insert_with(|| NegotiatedRecordCapability {
                max_read_schema_version: member.record_schema_version,
                max_write_schema_version: member.record_schema_version,
            });
    }
    let accepted_capabilities = NegotiatedCapabilities {
        protocol_version: SYNC_PROTOCOL_VERSION,
        record_kinds,
        max_transaction_members: DEFAULT_MAX_TRANSACTION_MEMBERS,
        max_transaction_bytes: DEFAULT_MAX_TRANSACTION_BYTES,
    };
    transaction
        .validate(0, &accepted_capabilities)
        .map_err(|_| ProtocolError::DecodedTransactionMismatch)?;

    let ReceiptDisposition::Accepted { advances } = &receipt.disposition else {
        return Err(ProtocolError::DecodedTransactionMismatch);
    };
    let expected_mutation_ids = transaction
        .members
        .iter()
        .map(|member| member.mutation_id.as_str())
        .collect::<BTreeSet<_>>();
    let receipt_mutation_ids = receipt
        .mutation_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if receipt.mutation_ids.len() != transaction.members.len()
        || receipt_mutation_ids.len() != receipt.mutation_ids.len()
        || receipt_mutation_ids != expected_mutation_ids
        || advances.len() != transaction.members.len()
    {
        return Err(ProtocolError::DecodedTransactionMismatch);
    }

    let mut advance_record_ids = BTreeSet::new();
    for advance in advances {
        if !advance_record_ids.insert(advance.record_id.as_str()) {
            return Err(ProtocolError::DecodedTransactionMismatch);
        }
        let member = transaction
            .members
            .iter()
            .find(|member| member.record_id == advance.record_id)
            .ok_or(ProtocolError::DecodedTransactionMismatch)?;
        if !advance_matches_member(advance, member) {
            return Err(ProtocolError::DecodedTransactionMismatch);
        }
    }
    Ok(advances)
}

fn advance_matches_member(advance: &HeadAdvance, member: &MutationEnvelope) -> bool {
    advance.record_id == member.record_id
        && advance.record_kind == member.record_kind
        && advance.record_schema_version == member.record_schema_version
        && advance.base_revision == member.base_head_revision
        && advance.base_version_id == member.base_head_version_id
        && advance.revision == member.proposed_revision
        && advance.version_id == member.version_id
        && advance.ciphertext_hash == member.ciphertext_hash
}

fn validate_bootstrap_head(record_id: &str, head: &AcceptedHead) -> Result<(), ProtocolError> {
    if !is_uuid_v7(record_id)
        || head.revision == 0
        || !is_uuid(&head.version_id)
        || !is_sha256(&head.ciphertext_hash)
        || head.authority_generation == 0
        || head.acceptance_checkpoint == 0
    {
        return Err(ProtocolError::BootstrapSnapshotInvalid);
    }
    Ok(())
}

fn validate_replica_head(record_id: &str, head: &ReplicaAcceptedHead) -> Result<(), ProtocolError> {
    if !is_uuid_v7(record_id)
        || head.revision == 0
        || !is_uuid(&head.version_id)
        || !is_sha256(&head.content_hash)
        || head.authority_generation == 0
    {
        return Err(ProtocolError::MalformedEnvelope);
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidCapability(String),
    UnsupportedProtocol,
    MalformedEnvelope,
    WrongLibrary,
    DeviceUnknown,
    DeviceRevoked,
    DeviceRegistrationMismatch,
    RecordKindReadOnly { kind: String, schema_version: u32 },
    RecordKindUnsupported { kind: String, schema_version: u32 },
    IncompleteTransaction,
    DuplicateTransactionMember,
    TransactionManifestMismatch,
    AggregateDigestMismatch,
    TransactionLimitExceeded,
    TransactionExpired,
    MutationIdReuse,
    TransactionIdReuse,
    CounterReuse,
    CounterGap { expected: u64, provided: u64 },
    PriorTransactionPending,
    TransactionUnknown,
    AuthorityGenerationStale { minimum: u64, provided: u64 },
    AuthorityGenerationAhead { current: u64, provided: u64 },
    PurgeGenerationStale { minimum: u64, provided: u64 },
    PurgeGenerationAhead { current: u64, provided: u64 },
    KeyEpochStale { minimum: u64, provided: u64 },
    KeyEpochAhead { current: u64, provided: u64 },
    CursorOverflow,
    CursorAhead { high_water: u64, provided: u64 },
    CursorGap { expected: u64, provided: u64 },
    InvalidPullLimit { maximum: u32, provided: u32 },
    BootstrapSnapshotInvalid,
    DownloadedSequenceReuse,
    DownloadedChangeMissing,
    QuarantineReceiptMismatch,
    DecodedTransactionMismatch,
    ReplicaBaseMismatch,
    CheckpointCorrupt(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000;

    fn id(number: u64) -> String {
        format!("00000000-0000-7000-8000-{number:012x}")
    }

    fn hash(label: &str) -> String {
        sha256_bytes(label.as_bytes())
    }

    fn notes_capability(read: u32, write: u32) -> ProtocolCapabilities {
        ProtocolCapabilities::new(
            1,
            1,
            BTreeMap::from([("note".to_string(), RecordKindCapability::new(read, write))]),
        )
    }

    fn authority(library_id: &str, devices: &[&str]) -> AuthorityState {
        let mut authority =
            AuthorityState::new(library_id.to_string(), 3, 2, 1, notes_capability(2, 2)).unwrap();
        for device in devices {
            authority
                .register_device((*device).to_string(), notes_capability(2, 2))
                .unwrap();
        }
        authority
    }

    #[allow(clippy::too_many_arguments)]
    fn transaction(
        library_id: &str,
        device_id: &str,
        counter: u64,
        transaction_id: String,
        mutation_id: String,
        record_id: String,
        base_revision: u64,
        base_version_id: Option<String>,
        version_id: String,
        body: &str,
    ) -> SignedTransaction {
        SignedTransaction::prepare(
            TransactionHeader {
                protocol_version: 1,
                library_id: library_id.to_string(),
                transaction_id,
                device_id: device_id.to_string(),
                device_transaction_counter: counter,
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 1,
            },
            vec![MutationDraft {
                mutation_id,
                operation: if base_revision == 0 {
                    MutationOperation::Create
                } else {
                    MutationOperation::Update
                },
                record_id,
                record_kind: "note".to_string(),
                record_schema_version: 1,
                base_head_revision: base_revision,
                base_head_version_id: base_version_id,
                proposed_revision: base_revision + 1,
                version_id,
                ciphertext: body.as_bytes().to_vec(),
            }],
            NOW + 100,
        )
        .unwrap()
        .attach_signatures(vec![vec![0x51; 64]])
        .unwrap()
    }

    fn terminal_receipt(outcome: SubmitOutcome) -> TransactionReceipt {
        match outcome {
            SubmitOutcome::Terminal(receipt) | SubmitOutcome::Replay(receipt) => receipt,
        }
    }

    fn accepted_change_at(authority: &AuthorityState, sequence: u64) -> AcceptedChange {
        authority
            .changes_after(sequence - 1, 1)
            .unwrap()
            .changes
            .into_iter()
            .next()
            .unwrap()
    }

    fn decoded_change(
        accepted_change: AcceptedChange,
        content_hashes: Vec<String>,
    ) -> DecodedChange {
        assert_eq!(
            accepted_change.transaction.members.len(),
            content_hashes.len()
        );
        let records = accepted_change
            .transaction
            .members
            .iter()
            .zip(content_hashes)
            .map(|(member, content_hash)| DecodedRecordAdvance {
                source_mutation_id: member.mutation_id.clone(),
                source_ciphertext_hash: member.ciphertext_hash.clone(),
                record_id: member.record_id.clone(),
                record_kind: member.record_kind.clone(),
                record_schema_version: member.record_schema_version,
                base_revision: member.base_head_revision,
                base_version_id: member.base_head_version_id.clone(),
                revision: member.proposed_revision,
                version_id: member.version_id.clone(),
                content_hash,
            })
            .collect();
        DecodedChange {
            accepted_change,
            records,
        }
    }

    #[test]
    fn negotiates_reader_and_writer_capabilities_per_kind() {
        let authority = notes_capability(2, 2);
        let old_writer = notes_capability(2, 1);
        let old_reader = notes_capability(1, 1);

        let negotiated = negotiate_capabilities(&authority, &old_writer).unwrap();
        assert_eq!(negotiated.access_for("note", 1), RecordAccess::ReadWrite);
        assert_eq!(negotiated.access_for("note", 2), RecordAccess::ReadOnly);
        assert_eq!(negotiated.access_for("meeting", 1), RecordAccess::Reject);

        let negotiated = negotiate_capabilities(&authority, &old_reader).unwrap();
        assert_eq!(negotiated.access_for("note", 2), RecordAccess::Reject);
    }

    #[test]
    fn transaction_signatures_are_attached_only_after_the_final_manifest_is_frozen() {
        let prepared = SignedTransaction::prepare(
            TransactionHeader {
                protocol_version: 1,
                library_id: id(1),
                transaction_id: id(2),
                device_id: id(3),
                device_transaction_counter: 1,
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 1,
            },
            vec![MutationDraft {
                mutation_id: id(4),
                operation: MutationOperation::Create,
                record_id: id(5),
                record_kind: "note".to_string(),
                record_schema_version: 1,
                base_head_revision: 0,
                base_head_version_id: None,
                proposed_revision: 1,
                version_id: id(6),
                ciphertext: b"ciphertext".to_vec(),
            }],
            NOW + 100,
        )
        .unwrap();

        let inputs = prepared.signing_inputs();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].mutation_id, id(4));
        assert_eq!(inputs[0].member_index, 0);
        let signable: serde_json::Value =
            serde_json::from_slice(&inputs[0].canonical_bytes).unwrap();
        assert_eq!(
            signable["mutation"]["transaction_manifest_digest"],
            prepared.manifest.digest()
        );
        assert_eq!(signable["mutation"]["signature"], json!([]));

        assert_eq!(
            prepared.clone().attach_signatures(Vec::new()),
            Err(ProtocolError::IncompleteTransaction)
        );
        assert_eq!(
            prepared.clone().attach_signatures(vec![Vec::new()]),
            Err(ProtocolError::MalformedEnvelope)
        );

        let signed = prepared.attach_signatures(vec![vec![0x44; 64]]).unwrap();
        assert_eq!(signed.members[0].signature, vec![0x44; 64]);
        assert_eq!(signed.members[0].signing_bytes(), inputs[0].canonical_bytes);
    }

    #[test]
    fn mutation_operation_is_required_and_uses_a_closed_wire_vocabulary() {
        assert_eq!(
            serde_json::to_value(MutationOperation::Create).unwrap(),
            json!("create")
        );
        assert_eq!(
            serde_json::to_value(MutationOperation::Update).unwrap(),
            json!("update")
        );
        assert_eq!(
            serde_json::to_value(MutationOperation::Delete).unwrap(),
            json!("delete")
        );

        let transaction = transaction(
            &id(20),
            &id(21),
            1,
            id(22),
            id(23),
            id(24),
            0,
            None,
            id(25),
            "ciphertext",
        );
        let mut value = serde_json::to_value(&transaction.members[0]).unwrap();
        value.as_object_mut().unwrap().remove("operation");
        assert!(serde_json::from_value::<MutationEnvelope>(value).is_err());

        let mut unknown = serde_json::to_value(&transaction.members[0]).unwrap();
        unknown["operation"] = json!("upsert");
        assert!(serde_json::from_value::<MutationEnvelope>(unknown).is_err());
    }

    #[test]
    fn prepare_enforces_operation_specific_revision_contracts() {
        let header = TransactionHeader {
            protocol_version: 1,
            library_id: id(30),
            transaction_id: id(31),
            device_id: id(32),
            device_transaction_counter: 1,
            authority_generation: 3,
            purge_generation: 2,
            key_epoch: 1,
        };
        let draft = |operation, base_head_revision, base_head_version_id, proposed_revision| {
            MutationDraft {
                mutation_id: id(33),
                operation,
                record_id: id(34),
                record_kind: "note".to_string(),
                record_schema_version: 1,
                base_head_revision,
                base_head_version_id,
                proposed_revision,
                version_id: id(35),
                ciphertext: b"ciphertext".to_vec(),
            }
        };

        for invalid in [
            draft(MutationOperation::Create, 1, Some(id(36)), 2),
            draft(MutationOperation::Create, 0, Some(id(36)), 1),
            draft(MutationOperation::Update, 0, None, 1),
            draft(MutationOperation::Update, 1, None, 2),
            draft(MutationOperation::Delete, 0, None, 1),
            draft(MutationOperation::Delete, 1, Some(id(36)), 3),
            draft(MutationOperation::Delete, u64::MAX, Some(id(36)), u64::MAX),
        ] {
            assert_eq!(
                SignedTransaction::prepare(header.clone(), vec![invalid], NOW + 100),
                Err(ProtocolError::MalformedEnvelope)
            );
        }

        for valid in [
            draft(MutationOperation::Create, 0, None, 1),
            draft(MutationOperation::Update, 1, Some(id(36)), 2),
            draft(MutationOperation::Delete, 1, Some(id(36)), 2),
        ] {
            SignedTransaction::prepare(header.clone(), vec![valid], NOW + 100).unwrap();
        }
    }

    #[test]
    fn operation_is_bound_by_member_manifest_and_signing_digests() {
        let create = transaction(
            &id(40),
            &id(41),
            1,
            id(42),
            id(43),
            id(44),
            0,
            None,
            id(45),
            "ciphertext",
        );
        let mut tampered = create.clone();
        tampered.members[0].operation = MutationOperation::Update;

        assert_ne!(
            create.members[0].member_digest(),
            tampered.members[0].member_digest()
        );
        assert_ne!(
            create.members[0].signing_bytes(),
            tampered.members[0].signing_bytes()
        );
        assert_ne!(
            create.members[0].signing_digest(),
            tampered.members[0].signing_digest()
        );
        assert_ne!(create.signed_digest(), tampered.signed_digest());

        let capabilities =
            negotiate_capabilities(&notes_capability(2, 2), &notes_capability(2, 2)).unwrap();
        assert_eq!(
            tampered.validate(NOW, &capabilities),
            Err(ProtocolError::AggregateDigestMismatch)
        );
    }

    #[test]
    fn transaction_manifest_is_order_independent_but_complete_and_digest_bound() {
        let library = id(1);
        let device = id(2);
        let mut transaction = SignedTransaction::prepare(
            TransactionHeader {
                protocol_version: 1,
                library_id: library,
                transaction_id: id(3),
                device_id: device,
                device_transaction_counter: 1,
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 1,
            },
            vec![
                MutationDraft {
                    mutation_id: id(4),
                    operation: MutationOperation::Create,
                    record_id: id(5),
                    record_kind: "note".to_string(),
                    record_schema_version: 1,
                    base_head_revision: 0,
                    base_head_version_id: None,
                    proposed_revision: 1,
                    version_id: id(6),
                    ciphertext: b"first".to_vec(),
                },
                MutationDraft {
                    mutation_id: id(7),
                    operation: MutationOperation::Create,
                    record_id: id(8),
                    record_kind: "note".to_string(),
                    record_schema_version: 1,
                    base_head_revision: 0,
                    base_head_version_id: None,
                    proposed_revision: 1,
                    version_id: id(9),
                    ciphertext: b"second".to_vec(),
                },
            ],
            NOW + 100,
        )
        .unwrap()
        .attach_signatures(vec![vec![1; 64], vec![2; 64]])
        .unwrap();
        let capabilities =
            negotiate_capabilities(&notes_capability(2, 2), &notes_capability(2, 2)).unwrap();

        transaction.members.reverse();
        transaction.validate(NOW, &capabilities).unwrap();

        let mut missing = transaction.clone();
        missing.members.pop();
        assert_eq!(
            missing.validate(NOW, &capabilities),
            Err(ProtocolError::IncompleteTransaction)
        );

        let mut bad_aggregate = transaction.clone();
        bad_aggregate.manifest.ordered_member_digests[0] = hash("wrong");
        let bad_manifest_digest = bad_aggregate.manifest.digest();
        for member in &mut bad_aggregate.members {
            member.transaction_manifest_digest = bad_manifest_digest.clone();
        }
        assert_eq!(
            bad_aggregate.validate(NOW, &capabilities),
            Err(ProtocolError::AggregateDigestMismatch)
        );

        let mut bad_commit = transaction;
        bad_commit
            .members
            .iter_mut()
            .find(|member| member.transaction_member_index == 0)
            .unwrap()
            .transaction_commit_marker = true;
        assert_eq!(
            bad_commit.validate(NOW, &capabilities),
            Err(ProtocolError::TransactionManifestMismatch)
        );
    }

    #[test]
    fn decoded_atomic_change_requires_exactly_one_record_for_every_accepted_member() {
        let library = id(10);
        let device = id(11);
        let mut authority = authority(&library, &[&device]);
        let transaction = SignedTransaction::prepare(
            TransactionHeader {
                protocol_version: SYNC_PROTOCOL_VERSION,
                library_id: library.clone(),
                transaction_id: id(12),
                device_id: device,
                device_transaction_counter: 1,
                authority_generation: 3,
                purge_generation: 2,
                key_epoch: 1,
            },
            vec![
                MutationDraft {
                    mutation_id: id(13),
                    operation: MutationOperation::Create,
                    record_id: id(14),
                    record_kind: "note".to_string(),
                    record_schema_version: 1,
                    base_head_revision: 0,
                    base_head_version_id: None,
                    proposed_revision: 1,
                    version_id: id(15),
                    ciphertext: b"first ciphertext".to_vec(),
                },
                MutationDraft {
                    mutation_id: id(16),
                    operation: MutationOperation::Create,
                    record_id: id(17),
                    record_kind: "note".to_string(),
                    record_schema_version: 1,
                    base_head_revision: 0,
                    base_head_version_id: None,
                    proposed_revision: 1,
                    version_id: id(18),
                    ciphertext: b"second ciphertext".to_vec(),
                },
            ],
            NOW + 100,
        )
        .unwrap()
        .attach_signatures(vec![vec![0x31; 64], vec![0x32; 64]])
        .unwrap();
        terminal_receipt(authority.submit_transaction(transaction, NOW).unwrap());
        let accepted = accepted_change_at(&authority, 1);

        let mut partial = decoded_change(
            accepted.clone(),
            vec![hash("first plaintext"), hash("second plaintext")],
        );
        partial.records.pop();
        let mut replica = ReplicaState::new(library.clone(), 3, 2).unwrap();
        replica
            .record_downloaded(1, accepted.transaction_digest.clone())
            .unwrap();

        let mut partial_receipt = accepted.clone();
        let ReceiptDisposition::Accepted { advances } = &mut partial_receipt.receipt.disposition
        else {
            unreachable!()
        };
        advances.pop();
        assert_eq!(
            replica.apply_decoded_change(decoded_change(
                partial_receipt,
                vec![hash("first plaintext"), hash("second plaintext")],
            )),
            Err(ProtocolError::DecodedTransactionMismatch)
        );
        assert_eq!(replica.applied_cursor(), 0);

        assert_eq!(
            replica.apply_decoded_change(partial),
            Err(ProtocolError::IncompleteTransaction)
        );
        assert_eq!(replica.applied_cursor(), 0);
        assert!(replica.accepted_heads.is_empty());

        let mut wrong_binding = decoded_change(
            accepted.clone(),
            vec![hash("first plaintext"), hash("second plaintext")],
        );
        wrong_binding.records[1].source_ciphertext_hash = hash("substituted ciphertext");
        assert_eq!(
            replica.apply_decoded_change(wrong_binding),
            Err(ProtocolError::DecodedTransactionMismatch)
        );
        assert_eq!(replica.applied_cursor(), 0);

        replica
            .apply_decoded_change(decoded_change(
                accepted,
                vec![hash("first plaintext"), hash("second plaintext")],
            ))
            .unwrap();
        assert_eq!(replica.applied_cursor(), 1);
        assert_eq!(replica.accepted_heads.len(), 2);
    }

    #[test]
    fn revocation_between_prepare_and_finish_terminally_cancels_transaction() {
        let library = id(181);
        let device = id(182);
        let transaction_id = id(183);
        let mut authority = authority(&library, &[&device]);
        let pending = transaction(
            &library,
            &device,
            1,
            transaction_id.clone(),
            id(184),
            id(185),
            0,
            None,
            id(186),
            "must not commit",
        );
        authority.begin_transaction(pending.clone(), NOW).unwrap();
        authority.revoke_device(&device).unwrap();

        let receipt = terminal_receipt(
            authority
                .finish_transaction(&transaction_id, NOW + 1)
                .unwrap(),
        );
        assert_eq!(
            receipt.disposition,
            ReceiptDisposition::Rejected {
                code: TerminalRejection::DeviceRevoked,
            }
        );
        assert_eq!(authority.high_water_cursor(), 0);
        assert!(authority.heads.is_empty());
        assert_eq!(
            authority.finish_transaction(&transaction_id, NOW + 2),
            Ok(SubmitOutcome::Replay(receipt))
        );
        assert_eq!(
            authority.begin_transaction(pending, NOW + 2),
            Ok(BeginOutcome::PendingReplay)
        );
    }

    #[test]
    fn atomic_conflict_receipt_terminates_every_mutation_branch() {
        let library = id(190);
        let mut replica = ReplicaState::new(library.clone(), 3, 2).unwrap();
        let first = replica
            .stage_local_branch(id(191), id(192), id(193), id(194), hash("first"))
            .unwrap();
        let second = replica
            .stage_local_branch(id(195), id(196), id(197), id(198), hash("second"))
            .unwrap();
        let receipt = TransactionReceipt {
            library_id: library,
            transaction_id: id(199),
            transaction_digest: hash("atomic conflict"),
            mutation_ids: vec![first.mutation_id.clone(), second.mutation_id.clone()],
            device_id: id(200),
            device_transaction_counter: 1,
            authority_generation: 3,
            purge_generation: 2,
            high_water_cursor: 0,
            disposition: ReceiptDisposition::Conflict {
                // Only the first member exposed the stale base. Atomic
                // rejection nevertheless terminates both pending branches.
                conflicts: vec![HeadConflict {
                    record_id: first.record_id.clone(),
                    proposed_version_id: first.version_id.clone(),
                    accepted_head: None,
                }],
            },
        };

        replica.observe_receipt(&receipt).unwrap();
        assert_eq!(
            replica.branch(&first.branch_id).unwrap().status,
            LocalBranchStatus::Conflict
        );
        assert_eq!(
            replica.branch(&second.branch_id).unwrap().status,
            LocalBranchStatus::Conflict
        );
    }

    #[test]
    fn pending_branch_never_masquerades_as_accepted_and_conflict_is_preserved() {
        let library = id(20);
        let record = id(21);
        let base_version = id(22);
        let mut replica = ReplicaState::new(library, 3, 2).unwrap();
        replica
            .seed_accepted_head(
                record.clone(),
                ReplicaAcceptedHead {
                    revision: 1,
                    version_id: base_version.clone(),
                    content_hash: hash("base"),
                    authority_generation: 3,
                    acceptance_checkpoint: 1,
                },
            )
            .unwrap();
        let branch = replica
            .stage_local_branch(id(23), id(24), record.clone(), id(25), hash("offline"))
            .unwrap();

        assert_eq!(replica.accepted_head(&record).unwrap().revision, 1);
        assert_eq!(branch.proposed_revision, 2);
        assert_eq!(branch.status, LocalBranchStatus::Pending);

        let receipt = TransactionReceipt {
            library_id: replica.library_id.clone(),
            transaction_id: id(26),
            transaction_digest: hash("tx"),
            mutation_ids: vec![branch.mutation_id.clone()],
            device_id: id(27),
            device_transaction_counter: 1,
            authority_generation: 3,
            purge_generation: 2,
            high_water_cursor: 2,
            disposition: ReceiptDisposition::Conflict {
                conflicts: vec![HeadConflict {
                    record_id: record.clone(),
                    proposed_version_id: branch.version_id.clone(),
                    accepted_head: None,
                }],
            },
        };
        replica.observe_receipt(&receipt).unwrap();
        assert_eq!(
            replica.branch(&branch.branch_id).unwrap().status,
            LocalBranchStatus::Conflict
        );
        assert_eq!(
            replica.accepted_head(&record).unwrap().version_id,
            base_version
        );
    }

    #[test]
    fn duplicate_and_reordered_offline_edits_converge_without_losing_rejected_branch() {
        let library = id(30);
        let device_a = id(31);
        let device_b = id(32);
        let record = id(33);
        let base_version = id(34);
        let version_a = id(35);
        let version_b = id(36);
        let mut authority = authority(&library, &[&device_a, &device_b]);

        let base = transaction(
            &library,
            &device_a,
            1,
            id(37),
            id(38),
            record.clone(),
            0,
            None,
            base_version.clone(),
            "base",
        );
        terminal_receipt(authority.submit_transaction(base, NOW).unwrap());

        let branch_a = transaction(
            &library,
            &device_a,
            2,
            id(39),
            id(40),
            record.clone(),
            1,
            Some(base_version.clone()),
            version_a.clone(),
            "from a",
        );
        let branch_b = transaction(
            &library,
            &device_b,
            1,
            id(41),
            id(42),
            record.clone(),
            1,
            Some(base_version),
            version_b.clone(),
            "from b",
        );

        let accepted_b =
            terminal_receipt(authority.submit_transaction(branch_b.clone(), NOW).unwrap());
        assert!(matches!(
            accepted_b.disposition,
            ReceiptDisposition::Accepted { .. }
        ));
        let replay_b = authority.submit_transaction(branch_b, NOW + 1).unwrap();
        assert_eq!(replay_b, SubmitOutcome::Replay(accepted_b.clone()));

        let rejected_a = terminal_receipt(authority.submit_transaction(branch_a, NOW).unwrap());
        assert!(matches!(
            rejected_a.disposition,
            ReceiptDisposition::Conflict { .. }
        ));
        assert_eq!(
            authority.accepted_head(&record).unwrap().version_id,
            version_b
        );
        assert_eq!(authority.high_water_cursor(), 2);
    }

    #[test]
    fn two_offline_replicas_resolve_a_stale_branch_and_converge() {
        let library = id(100);
        let device_a = id(101);
        let device_b = id(102);
        let record = id(103);
        let base_version = id(104);
        let mut authority = authority(&library, &[&device_a, &device_b]);
        let mut replica_a = ReplicaState::new(library.clone(), 3, 2).unwrap();
        let mut replica_b = ReplicaState::new(library.clone(), 3, 2).unwrap();

        let base_tx = transaction(
            &library,
            &device_a,
            1,
            id(105),
            id(106),
            record.clone(),
            0,
            None,
            base_version.clone(),
            "base ciphertext",
        );
        let base_receipt = terminal_receipt(authority.submit_transaction(base_tx, NOW).unwrap());
        let base_change = accepted_change_at(&authority, 1);
        for replica in [&mut replica_a, &mut replica_b] {
            replica
                .record_downloaded(1, base_receipt.transaction_digest.clone())
                .unwrap();
            replica
                .apply_decoded_change(decoded_change(
                    base_change.clone(),
                    vec![hash("base plaintext")],
                ))
                .unwrap();
        }

        let local_a = replica_a
            .stage_local_branch(id(107), id(108), record.clone(), id(109), hash("edit a"))
            .unwrap();
        let local_b = replica_b
            .stage_local_branch(id(110), id(111), record.clone(), id(112), hash("edit b"))
            .unwrap();
        let tx_a = transaction(
            &library,
            &device_a,
            2,
            id(113),
            local_a.mutation_id.clone(),
            record.clone(),
            local_a.base_revision,
            local_a.base_version_id.clone(),
            local_a.version_id.clone(),
            "edit a ciphertext",
        );
        let tx_b = transaction(
            &library,
            &device_b,
            1,
            id(114),
            local_b.mutation_id.clone(),
            record.clone(),
            local_b.base_revision,
            local_b.base_version_id.clone(),
            local_b.version_id.clone(),
            "edit b ciphertext",
        );

        // Delivery is reordered: B wins the compare-and-swap before A arrives.
        let accepted_b = terminal_receipt(authority.submit_transaction(tx_b, NOW + 1).unwrap());
        let accepted_b_change = accepted_change_at(&authority, 2);
        let conflict_a = terminal_receipt(authority.submit_transaction(tx_a, NOW + 1).unwrap());
        replica_a.observe_receipt(&conflict_a).unwrap();
        assert_eq!(
            replica_a.branch(&local_a.branch_id).unwrap().status,
            LocalBranchStatus::Conflict
        );

        for replica in [&mut replica_a, &mut replica_b] {
            replica
                .record_downloaded(2, accepted_b.transaction_digest.clone())
                .unwrap();
            replica
                .apply_decoded_change(decoded_change(
                    accepted_b_change.clone(),
                    vec![local_b.content_hash.clone()],
                ))
                .unwrap();
        }

        // A explicitly resolves its preserved branch against B's accepted head.
        let resolved = replica_a
            .stage_local_branch(id(115), id(116), record.clone(), id(117), hash("merged"))
            .unwrap();
        let resolved_tx = transaction(
            &library,
            &device_a,
            3,
            id(118),
            resolved.mutation_id.clone(),
            record.clone(),
            resolved.base_revision,
            resolved.base_version_id.clone(),
            resolved.version_id.clone(),
            "merged ciphertext",
        );
        let accepted_resolution =
            terminal_receipt(authority.submit_transaction(resolved_tx, NOW + 2).unwrap());
        let resolution_change = accepted_change_at(&authority, 3);
        for replica in [&mut replica_a, &mut replica_b] {
            replica
                .record_downloaded(3, accepted_resolution.transaction_digest.clone())
                .unwrap();
            replica
                .apply_decoded_change(decoded_change(
                    resolution_change.clone(),
                    vec![resolved.content_hash.clone()],
                ))
                .unwrap();
        }

        assert_eq!(
            replica_a.accepted_head(&record),
            replica_b.accepted_head(&record)
        );
        assert_eq!(replica_a.accepted_head(&record).unwrap().revision, 3);
        assert_eq!(
            replica_a.branch(&local_a.branch_id).unwrap().status,
            LocalBranchStatus::Conflict
        );
        assert_eq!(
            replica_a.branch(&resolved.branch_id).unwrap().status,
            LocalBranchStatus::Accepted
        );
    }

    #[test]
    fn mutation_transaction_and_counter_ids_are_bound_to_exact_signed_bytes() {
        let library = id(50);
        let device = id(51);
        let mut authority = authority(&library, &[&device]);
        let original = transaction(
            &library,
            &device,
            1,
            id(52),
            id(53),
            id(54),
            0,
            None,
            id(55),
            "original",
        );
        authority.begin_transaction(original.clone(), NOW).unwrap();

        let mut changed_transaction_id = original.clone();
        changed_transaction_id.members[0].signature[0] ^= 1;
        assert_eq!(
            authority.begin_transaction(changed_transaction_id, NOW),
            Err(ProtocolError::TransactionIdReuse)
        );

        authority.finish_transaction(&id(52), NOW).unwrap();
        let mutation_reuse = transaction(
            &library,
            &device,
            2,
            id(56),
            id(53),
            id(57),
            0,
            None,
            id(58),
            "different",
        );
        assert_eq!(
            authority.begin_transaction(mutation_reuse, NOW),
            Err(ProtocolError::MutationIdReuse)
        );

        let counter_reuse = transaction(
            &library,
            &device,
            1,
            id(59),
            id(60),
            id(61),
            0,
            None,
            id(62),
            "counter rollback",
        );
        assert_eq!(
            authority.begin_transaction(counter_reuse, NOW),
            Err(ProtocolError::CounterReuse)
        );
    }

    #[test]
    fn prepared_and_terminal_transactions_replay_across_crash_restart() {
        let library = id(70);
        let device = id(71);
        let mut authority = authority(&library, &[&device]);
        let transaction = transaction(
            &library,
            &device,
            1,
            id(72),
            id(73),
            id(74),
            0,
            None,
            id(75),
            "durable",
        );

        assert_eq!(
            authority
                .begin_transaction(transaction.clone(), NOW)
                .unwrap(),
            BeginOutcome::Prepared
        );
        let checkpoint = authority.checkpoint_json().unwrap();
        let mut restarted = AuthorityState::from_checkpoint_json(&checkpoint).unwrap();
        assert_eq!(
            restarted
                .begin_transaction(transaction.clone(), NOW + 1)
                .unwrap(),
            BeginOutcome::PendingReplay
        );
        let accepted = terminal_receipt(restarted.finish_transaction(&id(72), NOW + 1).unwrap());

        let terminal_checkpoint = restarted.checkpoint_json().unwrap();
        let mut restarted_again =
            AuthorityState::from_checkpoint_json(&terminal_checkpoint).unwrap();
        assert_eq!(
            restarted_again
                .submit_transaction(transaction, NOW + 2)
                .unwrap(),
            SubmitOutcome::Replay(accepted)
        );
    }

    #[test]
    fn checkpoint_rejects_an_invalid_operation_even_if_internal_digests_are_rebound() {
        let library = id(80);
        let device = id(81);
        let transaction_id = id(82);
        let mutation_id = id(83);
        let mut authority = authority(&library, &[&device]);
        let original = transaction(
            &library,
            &device,
            1,
            transaction_id.clone(),
            mutation_id.clone(),
            id(84),
            0,
            None,
            id(85),
            "durable",
        );
        authority.begin_transaction(original, NOW).unwrap();

        let rebound_digest = {
            let record = authority.transactions.get_mut(&transaction_id).unwrap();
            record.transaction.members[0].operation = MutationOperation::Update;
            refresh_manifest(&mut record.transaction);
            record.signed_digest = record.transaction.signed_digest();
            record.signed_digest.clone()
        };
        authority.mutation_bindings.insert(
            mutation_id,
            authority.transactions[&transaction_id].transaction.members[0].signed_digest(),
        );
        authority
            .counters
            .get_mut(&device)
            .unwrap()
            .bindings
            .insert(1, rebound_digest);

        assert!(matches!(
            AuthorityState::from_checkpoint_json(&authority.checkpoint_json().unwrap()),
            Err(ProtocolError::CheckpointCorrupt(_))
        ));
    }

    #[test]
    fn accepted_change_pages_are_bounded_contiguous_and_duplicate_free() {
        let library = id(300);
        let device = id(301);
        let mut authority = authority(&library, &[&device]);
        for offset in 0..5_u64 {
            let accepted = authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        offset + 1,
                        id(310 + offset * 4),
                        id(311 + offset * 4),
                        id(312 + offset * 4),
                        0,
                        None,
                        id(313 + offset * 4),
                        &format!("opaque {offset}"),
                    ),
                    NOW,
                )
                .unwrap();
            assert!(matches!(
                terminal_receipt(accepted).disposition,
                ReceiptDisposition::Accepted { .. }
            ));
        }

        let first = authority.changes_after(0, 2).unwrap();
        assert_eq!(
            first
                .changes
                .iter()
                .map(|change| change.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(first.next_cursor, 2);
        assert!(first.has_more);

        let second = authority.changes_after(first.next_cursor, 2).unwrap();
        let third = authority.changes_after(second.next_cursor, 2).unwrap();
        let sequences = first
            .changes
            .iter()
            .chain(&second.changes)
            .chain(&third.changes)
            .map(|change| change.sequence)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![1, 2, 3, 4, 5]);
        assert_eq!(sequences.iter().copied().collect::<BTreeSet<_>>().len(), 5);
        assert_eq!(third.next_cursor, 5);
        assert!(!third.has_more);

        let empty = authority.changes_after(5, 1).unwrap();
        assert!(empty.changes.is_empty());
        assert_eq!(empty.next_cursor, 5);
        assert_eq!(
            authority.changes_after(6, 1),
            Err(ProtocolError::CursorAhead {
                high_water: 5,
                provided: 6,
            })
        );
        assert_eq!(
            authority.changes_after(0, 0),
            Err(ProtocolError::InvalidPullLimit {
                maximum: MAX_PULL_PAGE_CHANGES,
                provided: 0,
            })
        );
        assert_eq!(
            authority.changes_after(0, MAX_PULL_PAGE_CHANGES + 1),
            Err(ProtocolError::InvalidPullLimit {
                maximum: MAX_PULL_PAGE_CHANGES,
                provided: MAX_PULL_PAGE_CHANGES + 1,
            })
        );
    }

    #[test]
    fn accepted_pull_log_and_bootstrap_snapshot_survive_restart_exactly() {
        let library = id(400);
        let device = id(401);
        let mut authority = authority(&library, &[&device]);
        for offset in 0..2_u64 {
            terminal_receipt(
                authority
                    .submit_transaction(
                        transaction(
                            &library,
                            &device,
                            offset + 1,
                            id(410 + offset * 4),
                            id(411 + offset * 4),
                            id(412 + offset * 4),
                            0,
                            None,
                            id(413 + offset * 4),
                            &format!("ciphertext {offset}"),
                        ),
                        NOW,
                    )
                    .unwrap(),
            );
        }
        let expected_page = authority.changes_after(0, 10).unwrap();
        let expected_snapshot = authority.bootstrap_snapshot().unwrap();
        let checkpoint = authority.checkpoint_json().unwrap();

        let restarted = AuthorityState::from_checkpoint_json(&checkpoint).unwrap();
        assert_eq!(restarted.changes_after(0, 10).unwrap(), expected_page);
        assert_eq!(restarted.bootstrap_snapshot().unwrap(), expected_snapshot);
        expected_snapshot.validate().unwrap();
    }

    #[test]
    fn bootstrap_snapshot_is_frozen_across_a_concurrent_later_write() {
        let library = id(500);
        let device = id(501);
        let record = id(502);
        let first_version = id(503);
        let mut authority = authority(&library, &[&device]);
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        1,
                        id(504),
                        id(505),
                        record.clone(),
                        0,
                        None,
                        first_version.clone(),
                        "first opaque version",
                    ),
                    NOW,
                )
                .unwrap(),
        );
        let frozen = authority.bootstrap_snapshot().unwrap();
        let frozen_digest = frozen.checkpoint_digest.clone();

        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        2,
                        id(506),
                        id(507),
                        record.clone(),
                        1,
                        Some(first_version),
                        id(508),
                        "second opaque version",
                    ),
                    NOW + 1,
                )
                .unwrap(),
        );
        let current = authority.bootstrap_snapshot().unwrap();

        frozen.validate().unwrap();
        current.validate().unwrap();
        assert_eq!(frozen.high_water_cursor, 1);
        assert_eq!(frozen.records[0].accepted_head.revision, 1);
        assert_eq!(frozen.checkpoint_digest, frozen_digest);
        assert_eq!(current.high_water_cursor, 2);
        assert_eq!(current.records[0].accepted_head.revision, 2);
        assert_ne!(current.checkpoint_digest, frozen.checkpoint_digest);
    }

    #[test]
    fn delete_operation_survives_manifest_log_bootstrap_and_authority_restart() {
        let library = id(520);
        let device = id(521);
        let record = id(522);
        let first_version = id(523);
        let mut authority = authority(&library, &[&device]);
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        1,
                        id(524),
                        id(525),
                        record.clone(),
                        0,
                        None,
                        first_version.clone(),
                        "created ciphertext",
                    ),
                    NOW,
                )
                .unwrap(),
        );

        let mut deletion = transaction(
            &library,
            &device,
            2,
            id(526),
            id(527),
            record,
            1,
            Some(first_version),
            id(528),
            "tombstone ciphertext",
        );
        deletion.members[0].operation = MutationOperation::Delete;
        refresh_manifest(&mut deletion);
        terminal_receipt(
            authority
                .submit_transaction(deletion.clone(), NOW + 1)
                .unwrap(),
        );

        let change = accepted_change_at(&authority, 2);
        assert_eq!(
            change.transaction.members[0].operation,
            MutationOperation::Delete
        );
        assert_eq!(
            authority.bootstrap_snapshot().unwrap().records[0]
                .mutation
                .operation,
            MutationOperation::Delete
        );

        let checkpoint = authority.checkpoint_json().unwrap();
        let restarted = AuthorityState::from_checkpoint_json(&checkpoint).unwrap();
        assert_eq!(
            restarted.bootstrap_snapshot().unwrap().records[0]
                .mutation
                .operation,
            MutationOperation::Delete
        );

        let mut tampered: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
        tampered["transactions"][id(526)]["transaction"]["members"][0]["operation"] =
            json!("update");
        assert!(matches!(
            AuthorityState::from_checkpoint_json(&serde_json::to_string(&tampered).unwrap()),
            Err(ProtocolError::CheckpointCorrupt(_))
        ));

        let snapshot = authority.bootstrap_snapshot().unwrap();
        let mut tampered_snapshot = snapshot.clone();
        tampered_snapshot.records[0].mutation.operation = MutationOperation::Update;
        assert_ne!(
            snapshot.checkpoint_digest,
            tampered_snapshot.computed_checkpoint_digest()
        );
    }

    #[test]
    fn bootstrap_rejects_rehashed_but_structurally_invalid_envelopes() {
        let library = id(550);
        let device = id(551);
        let mut authority = authority(&library, &[&device]);
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        1,
                        id(552),
                        id(553),
                        id(554),
                        0,
                        None,
                        id(555),
                        "bootstrap ciphertext",
                    ),
                    NOW,
                )
                .unwrap(),
        );
        let snapshot = authority.bootstrap_snapshot().unwrap();

        let mut bad_checkpoint = snapshot.clone();
        bad_checkpoint.records[0]
            .accepted_head
            .acceptance_checkpoint = 0;
        bad_checkpoint.checkpoint_digest = bad_checkpoint.computed_checkpoint_digest();
        assert_eq!(
            bad_checkpoint.validate(),
            Err(ProtocolError::BootstrapSnapshotInvalid)
        );

        let mut bad_generation = snapshot.clone();
        bad_generation.records[0].mutation.authority_generation = 4;
        bad_generation.records[0].accepted_head.authority_generation = 4;
        bad_generation.checkpoint_digest = bad_generation.computed_checkpoint_digest();
        assert_eq!(
            bad_generation.validate(),
            Err(ProtocolError::BootstrapSnapshotInvalid)
        );

        let mut bad_manifest_member = snapshot.clone();
        bad_manifest_member.records[0]
            .mutation
            .transaction_commit_marker = false;
        bad_manifest_member.checkpoint_digest = bad_manifest_member.computed_checkpoint_digest();
        assert_eq!(
            bad_manifest_member.validate(),
            Err(ProtocolError::BootstrapSnapshotInvalid)
        );

        let mut bad_revision = snapshot;
        bad_revision.records[0].mutation.base_head_revision = u64::MAX;
        bad_revision.records[0].mutation.proposed_revision = u64::MAX;
        bad_revision.records[0].accepted_head.revision = u64::MAX;
        bad_revision.checkpoint_digest = bad_revision.computed_checkpoint_digest();
        assert_eq!(
            bad_revision.validate(),
            Err(ProtocolError::BootstrapSnapshotInvalid)
        );

        let mut bad_operation = authority.bootstrap_snapshot().unwrap();
        bad_operation.records[0].mutation.operation = MutationOperation::Update;
        bad_operation.checkpoint_digest = bad_operation.computed_checkpoint_digest();
        assert_eq!(
            bad_operation.validate(),
            Err(ProtocolError::BootstrapSnapshotInvalid)
        );
    }

    #[test]
    fn decoded_revision_overflow_is_rejected_without_advancing_cursor() {
        let library = id(560);
        let device = id(561);
        let mut authority = authority(&library, &[&device]);
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        1,
                        id(562),
                        id(563),
                        id(564),
                        0,
                        None,
                        id(565),
                        "valid ciphertext",
                    ),
                    NOW,
                )
                .unwrap(),
        );
        let accepted = accepted_change_at(&authority, 1);
        let mut decoded = decoded_change(accepted.clone(), vec![hash("valid plaintext")]);
        decoded.records[0].base_revision = u64::MAX;
        decoded.records[0].revision = u64::MAX;

        let mut replica = ReplicaState::new(library, 3, 2).unwrap();
        replica
            .record_downloaded(1, accepted.transaction_digest)
            .unwrap();
        assert_eq!(
            replica.apply_decoded_change(decoded),
            Err(ProtocolError::MalformedEnvelope)
        );
        assert_eq!(replica.applied_cursor(), 0);
        assert!(replica.accepted_heads.is_empty());
    }

    #[test]
    fn conflicts_and_terminal_rejections_never_enter_the_accepted_change_feed() {
        let library = id(600);
        let device_a = id(601);
        let device_b = id(602);
        let record = id(603);
        let base_version = id(604);
        let mut authority = authority(&library, &[&device_a, &device_b]);
        let base_id = id(605);
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device_a,
                        1,
                        base_id.clone(),
                        id(606),
                        record.clone(),
                        0,
                        None,
                        base_version.clone(),
                        "base",
                    ),
                    NOW,
                )
                .unwrap(),
        );

        let accepted_id = id(607);
        let winning = transaction(
            &library,
            &device_b,
            1,
            accepted_id.clone(),
            id(608),
            record.clone(),
            1,
            Some(base_version.clone()),
            id(609),
            "winner",
        );
        terminal_receipt(authority.submit_transaction(winning, NOW).unwrap());

        let conflict_id = id(610);
        let conflict = terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device_a,
                        2,
                        conflict_id.clone(),
                        id(611),
                        record,
                        1,
                        Some(base_version),
                        id(612),
                        "loser",
                    ),
                    NOW,
                )
                .unwrap(),
        );
        assert!(matches!(
            conflict.disposition,
            ReceiptDisposition::Conflict { .. }
        ));

        let rejected_id = id(613);
        let expired = transaction(
            &library,
            &device_a,
            3,
            rejected_id.clone(),
            id(614),
            id(615),
            0,
            None,
            id(616),
            "expired",
        );
        authority.begin_transaction(expired, NOW).unwrap();
        let rejected = terminal_receipt(
            authority
                .finish_transaction(&rejected_id, NOW + 101)
                .unwrap(),
        );
        assert_eq!(
            rejected.disposition,
            ReceiptDisposition::Rejected {
                code: TerminalRejection::Expired,
            }
        );

        let page = authority.changes_after(0, 10).unwrap();
        assert_eq!(page.high_water_cursor, 2);
        assert_eq!(page.changes.len(), 2);
        let accepted_ids = page
            .changes
            .iter()
            .map(|change| change.transaction.manifest.transaction_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            accepted_ids,
            BTreeSet::from([base_id.as_str(), accepted_id.as_str()])
        );
        assert!(!accepted_ids.contains(conflict_id.as_str()));
        assert!(!accepted_ids.contains(rejected_id.as_str()));
    }

    #[test]
    fn poison_requires_bound_quarantine_receipt_then_next_cursor_can_apply() {
        let library = id(80);
        let record = id(81);
        let device = id(82);
        let mut authority = authority(&library, &[&device]);
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        1,
                        id(83),
                        id(84),
                        id(85),
                        0,
                        None,
                        id(86),
                        "poison ciphertext",
                    ),
                    NOW,
                )
                .unwrap(),
        );
        terminal_receipt(
            authority
                .submit_transaction(
                    transaction(
                        &library,
                        &device,
                        2,
                        id(87),
                        id(88),
                        record.clone(),
                        0,
                        None,
                        id(89),
                        "good ciphertext",
                    ),
                    NOW,
                )
                .unwrap(),
        );
        let poison_change = accepted_change_at(&authority, 1);
        let good_change = accepted_change_at(&authority, 2);
        let poison_digest = poison_change.transaction_digest.clone();
        let good_digest = good_change.transaction_digest.clone();
        let mut replica = ReplicaState::new(library.clone(), 3, 2).unwrap();
        replica.record_downloaded(2, good_digest.clone()).unwrap();
        assert_eq!(replica.downloaded_cursor(), 0);
        replica.record_downloaded(1, poison_digest.clone()).unwrap();
        assert_eq!(replica.downloaded_cursor(), 2);
        assert_eq!(replica.applied_cursor(), 0);

        assert_eq!(
            AuthenticatedQuarantineReceipt::from_verified_authority_receipt(
                library.clone(),
                1,
                poison_digest.clone(),
                3,
                2,
                hash("evidence"),
                "   ".to_string(),
            ),
            Err(ProtocolError::MalformedEnvelope)
        );

        let wrong = AuthenticatedQuarantineReceipt::from_verified_authority_receipt(
            library.clone(),
            1,
            hash("wrong"),
            3,
            2,
            hash("evidence"),
            "invalid_record".to_string(),
        )
        .unwrap();
        assert_eq!(
            replica.quarantine_poison(wrong),
            Err(ProtocolError::QuarantineReceiptMismatch)
        );
        assert_eq!(replica.applied_cursor(), 0);

        let cross_library = AuthenticatedQuarantineReceipt::from_verified_authority_receipt(
            id(890),
            1,
            poison_digest.clone(),
            3,
            2,
            hash("evidence"),
            "invalid_record".to_string(),
        )
        .unwrap();
        assert_eq!(
            replica.quarantine_poison(cross_library),
            Err(ProtocolError::QuarantineReceiptMismatch)
        );
        assert_eq!(replica.applied_cursor(), 0);

        let receipt = AuthenticatedQuarantineReceipt::from_verified_authority_receipt(
            library,
            1,
            poison_digest,
            3,
            2,
            hash("evidence"),
            "invalid_record".to_string(),
        )
        .unwrap();
        replica.quarantine_poison(receipt).unwrap();
        assert_eq!(replica.applied_cursor(), 1);
        assert!(replica.quarantined_change(1).is_some());

        replica
            .apply_decoded_change(decoded_change(good_change, vec![hash("safe")]))
            .unwrap();
        assert_eq!(replica.applied_cursor(), 2);
        assert_eq!(replica.accepted_head(&record).unwrap().revision, 1);
        replica.record_downloaded(2, good_digest).unwrap();
        assert_eq!(
            replica.record_downloaded(2, hash("equivocated bytes")),
            Err(ProtocolError::DownloadedSequenceReuse)
        );
    }

    #[test]
    fn stale_authority_purge_and_key_generations_fail_closed() {
        let library = id(90);
        let device = id(91);
        let mut authority =
            AuthorityState::new(library.clone(), 3, 2, 2, notes_capability(2, 2)).unwrap();
        authority
            .register_device(device.clone(), notes_capability(2, 2))
            .unwrap();
        let original = transaction(
            &library,
            &device,
            1,
            id(92),
            id(93),
            id(94),
            0,
            None,
            id(95),
            "stale",
        );

        let mut stale_authority = original.clone();
        stale_authority.manifest.authority_generation = 2;
        for member in &mut stale_authority.members {
            member.authority_generation = 2;
        }
        refresh_manifest(&mut stale_authority);
        assert_eq!(
            authority.begin_transaction(stale_authority, NOW),
            Err(ProtocolError::AuthorityGenerationStale {
                minimum: 3,
                provided: 2,
            })
        );

        let mut stale_purge = original.clone();
        stale_purge.manifest.purge_generation = 1;
        for member in &mut stale_purge.members {
            member.purge_generation = 1;
        }
        refresh_manifest(&mut stale_purge);
        assert_eq!(
            authority.begin_transaction(stale_purge, NOW),
            Err(ProtocolError::PurgeGenerationStale {
                minimum: 2,
                provided: 1,
            })
        );

        let stale_key = original;
        assert_eq!(
            authority.begin_transaction(stale_key, NOW),
            Err(ProtocolError::KeyEpochStale {
                minimum: 2,
                provided: 1,
            })
        );
    }

    fn refresh_manifest(transaction: &mut SignedTransaction) {
        let mut members = transaction.members.iter().collect::<Vec<_>>();
        members.sort_by_key(|member| member.transaction_member_index);
        transaction.manifest.ordered_member_digests = members
            .into_iter()
            .map(MutationEnvelope::member_digest)
            .collect();
        let digest = transaction.manifest.digest();
        for member in &mut transaction.members {
            member.transaction_manifest_digest = digest.clone();
        }
    }
}
