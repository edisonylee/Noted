//! Durable SQLite adapter for the phone-side exact-request actor.
//!
//! The actor owns protocol ordering and verification; [`MobileStore`] owns the
//! crash boundary. This adapter deliberately exposes neither SQLite nor URLs
//! and reconstructs every active profile from the authenticated activation.

use crate::{
    direct_sync::DirectEndpoint,
    mobile_store::{
        MobileAuthorityRevocationEvidence, MobileAuthorityRevocationResult,
        MobileDirectSyncPushDisposition, MobileDirectSyncRequest, MobileDirectSyncRequestDraft,
        MobileStore,
    },
    mobile_sync_runtime::{
        ActiveSyncProfile, AuthenticatedResponseWire, ExactRequestCompletion, ExactRequestJournal,
        ExactRequestPurpose, ExactRequestState, JournaledExactRequest, MobileSyncRuntimeError,
    },
    pairing_protocol::Invitation,
    portable::{canonical_json, canonical_sha256},
};

const SHA256_BYTES: usize = 32;

pub struct MobileStoreExactRequestJournal<'a> {
    store: &'a MobileStore,
}

impl<'a> MobileStoreExactRequestJournal<'a> {
    pub const fn new(store: &'a MobileStore) -> Self {
        Self { store }
    }

    /// Push receipt semantics are intentionally explicit. A generic request
    /// completion cannot decide whether an accepted upload is still awaiting
    /// its authoritative pull echo or whether a branch must be preserved.
    pub fn complete_push(
        &self,
        completion: &ExactRequestCompletion,
        disposition: MobileDirectSyncPushDisposition,
        error_code: Option<&str>,
    ) -> Result<(), MobileSyncRuntimeError> {
        if completion.endpoint != DirectEndpoint::Push {
            return Err(MobileSyncRuntimeError::RecoveryEndpointMismatch);
        }
        let request = self.unresolved_for_completion(completion)?;
        if request.endpoint != DirectEndpoint::Push {
            return Err(MobileSyncRuntimeError::RecoveryEndpointMismatch);
        }
        self.store
            .complete_direct_sync_push_request(&completion.request_id, disposition, error_code)
            .map_err(store_error)?;
        Ok(())
    }

    pub fn quarantine(
        &self,
        completion: &ExactRequestCompletion,
        error_code: &str,
    ) -> Result<(), MobileSyncRuntimeError> {
        let request = self.unresolved_for_completion(completion)?;
        self.store
            .quarantine_direct_sync_request(
                &completion.request_id,
                request.endpoint.path(),
                error_code,
            )
            .map_err(store_error)?;
        Ok(())
    }

    pub fn apply_authority_revocation(
        &self,
        request_id: &str,
        endpoint: DirectEndpoint,
        exact_response_bytes: &[u8],
    ) -> Result<MobileAuthorityRevocationResult, MobileSyncRuntimeError> {
        self.store
            .apply_authority_revocation(&MobileAuthorityRevocationEvidence {
                request_id: request_id.to_owned(),
                endpoint: endpoint.path().to_owned(),
                exact_response_bytes: exact_response_bytes.to_vec(),
            })
            .map_err(store_error)
    }

    fn unresolved_for_completion(
        &self,
        completion: &ExactRequestCompletion,
    ) -> Result<JournaledExactRequest, MobileSyncRuntimeError> {
        let request = self
            .unresolved_exact_request()?
            .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
        if request.request_id != completion.request_id
            || request.endpoint != completion.endpoint
            || request.request_body_sha256 != completion.request_body_sha256
        {
            return Err(MobileSyncRuntimeError::JournalCorrupt);
        }
        Ok(request)
    }
}

impl ExactRequestJournal for MobileStoreExactRequestJournal<'_> {
    fn active_sync_profile(&self) -> Result<ActiveSyncProfile, MobileSyncRuntimeError> {
        let activation = self
            .store
            .finalized_pairing_activation()
            .map_err(store_error)?
            .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
        let invitation: Invitation =
            serde_json::from_slice(&activation.checkpoint.client.invitation_bytes)
                .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?;
        let activation_value = serde_json::to_value(&activation)
            .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?;
        let profile = ActiveSyncProfile {
            identity_handle: activation.checkpoint.identity_handle.clone(),
            receipt_id: activation.receipt_id.clone(),
            activation_sha256: canonical_sha256(&activation_value),
            library_id: activation.library_id.clone(),
            device_id: activation.device_id.clone(),
            default_scope_id: activation.default_scope_id.clone(),
            authority_generation: positive_u64(activation.authority_generation)?,
            purge_generation: nonnegative_u64(activation.purge_generation)?,
            key_epoch: positive_u64(activation.key_epoch)?,
            environment: activation.checkpoint.client.config.environment,
            library_data_class: activation.checkpoint.client.config.library_data_class,
            durable_sync_spki_sha256: fixed_bytes(&activation.sync_spki_sha256)?,
            device_signing_public_key: activation
                .checkpoint
                .client
                .identity
                .signing_public_key
                .clone(),
            authority_signing_public_key: invitation.authority_signing_public_key,
            granted_scopes: activation.granted_scopes,
            capabilities: activation.capabilities,
            revoked: false,
        };
        profile.validate_fixture()?;
        Ok(profile)
    }

    fn unresolved_exact_request(
        &self,
    ) -> Result<Option<JournaledExactRequest>, MobileSyncRuntimeError> {
        let rows = self
            .store
            .recover_direct_sync_requests()
            .map_err(store_error)?;
        if rows.len() > 1 {
            return Err(MobileSyncRuntimeError::JournalCorrupt);
        }
        rows.into_iter().next().map(map_request).transpose()
    }

    fn prepare_exact_request(
        &mut self,
        request: JournaledExactRequest,
    ) -> Result<(), MobileSyncRuntimeError> {
        if request.state != ExactRequestState::AwaitingResponse
            || request.attempt_count != 0
            || request.response.is_some()
        {
            return Err(MobileSyncRuntimeError::JournalCorrupt);
        }
        let purpose_value = serde_json::to_value(&request.purpose)
            .map_err(|_| MobileSyncRuntimeError::InvalidSemanticReference)?;
        let purpose_json = canonical_json(&purpose_value).into_bytes();
        let (push_transaction_id, push_counter) = match &request.purpose {
            ExactRequestPurpose::Push {
                transaction_id,
                device_transaction_counter,
                ..
            } => (
                Some(transaction_id.clone()),
                Some(
                    i64::try_from(*device_transaction_counter)
                        .map_err(|_| MobileSyncRuntimeError::InvalidSemanticReference)?,
                ),
            ),
            _ => (None, None),
        };
        let result = self
            .store
            .prepare_direct_sync_request(&MobileDirectSyncRequestDraft {
                request_id: request.request_id.clone(),
                endpoint: request.endpoint.path().to_owned(),
                operation: operation_name(request.endpoint).to_owned(),
                purpose_json,
                push_transaction_id,
                push_counter,
                signed_request_bytes: request.request_body.clone(),
            })
            .map_err(store_error)?;
        let stored = map_request(result.request)?;
        if stored != request {
            return Err(MobileSyncRuntimeError::JournalCorrupt);
        }
        Ok(())
    }

    fn record_transport_attempt(
        &mut self,
        request_id: &str,
        exact_request_sha256: [u8; SHA256_BYTES],
    ) -> Result<(), MobileSyncRuntimeError> {
        let request = self
            .unresolved_exact_request()?
            .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
        require_request_identity(&request, request_id, exact_request_sha256)?;
        self.store
            .record_direct_sync_attempt(request_id, request.endpoint.path())
            .map_err(store_error)?;
        Ok(())
    }

    fn store_authenticated_response(
        &mut self,
        request_id: &str,
        exact_request_sha256: [u8; SHA256_BYTES],
        response: AuthenticatedResponseWire,
    ) -> Result<(), MobileSyncRuntimeError> {
        let request = self
            .unresolved_exact_request()?
            .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
        require_request_identity(&request, request_id, exact_request_sha256)?;
        self.store
            .record_direct_sync_response(
                request_id,
                request.endpoint.path(),
                response.status,
                &response.content_type,
                &response.body,
            )
            .map_err(store_error)?;
        Ok(())
    }

    fn complete_exact_request(
        &mut self,
        request_id: &str,
        exact_request_sha256: [u8; SHA256_BYTES],
    ) -> Result<(), MobileSyncRuntimeError> {
        let request = self
            .unresolved_exact_request()?
            .ok_or(MobileSyncRuntimeError::StateUnavailable)?;
        require_request_identity(&request, request_id, exact_request_sha256)?;
        if request.endpoint == DirectEndpoint::Push {
            return Err(MobileSyncRuntimeError::StateUnavailable);
        }
        self.store
            .complete_direct_sync_request(request_id, request.endpoint.path())
            .map_err(store_error)?;
        Ok(())
    }
}

fn map_request(
    row: MobileDirectSyncRequest,
) -> Result<JournaledExactRequest, MobileSyncRuntimeError> {
    let endpoint = endpoint_from_path(&row.endpoint)?;
    if row.operation != operation_name(endpoint) {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    let purpose_value: serde_json::Value = serde_json::from_slice(&row.purpose_json)
        .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?;
    if canonical_json(&purpose_value).as_bytes() != row.purpose_json {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    let purpose: ExactRequestPurpose = serde_json::from_value(purpose_value)
        .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?;
    if purpose.endpoint() != endpoint {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    let state = match row.state.as_str() {
        "pending" => ExactRequestState::AwaitingResponse,
        "response_received" => ExactRequestState::ResponseStored,
        _ => return Err(MobileSyncRuntimeError::JournalCorrupt),
    };
    let response = match (
        row.response_status,
        row.response_content_type,
        row.response_bytes,
        row.response_sha256,
    ) {
        (None, None, None, None) => None,
        (Some(status), Some(content_type), Some(body), Some(body_sha256)) => {
            Some(AuthenticatedResponseWire {
                status: u16::try_from(status)
                    .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?,
                content_type,
                body,
                body_sha256: decode_sha256_hex(&body_sha256)?,
            })
        }
        _ => return Err(MobileSyncRuntimeError::JournalCorrupt),
    };
    Ok(JournaledExactRequest {
        endpoint,
        request_id: row.request_id,
        purpose,
        request_body: row.request_bytes,
        request_body_sha256: decode_sha256_hex(&row.request_sha256)?,
        state,
        attempt_count: u32::try_from(row.attempts)
            .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)?,
        response,
    })
}

fn require_request_identity(
    request: &JournaledExactRequest,
    request_id: &str,
    digest: [u8; SHA256_BYTES],
) -> Result<(), MobileSyncRuntimeError> {
    if request.request_id != request_id || request.request_body_sha256 != digest {
        Err(MobileSyncRuntimeError::JournalCorrupt)
    } else {
        Ok(())
    }
}

fn endpoint_from_path(path: &str) -> Result<DirectEndpoint, MobileSyncRuntimeError> {
    match path {
        "/sync/v1/negotiate" => Ok(DirectEndpoint::Negotiate),
        "/sync/v1/bootstrap" => Ok(DirectEndpoint::Bootstrap),
        "/sync/v1/push" => Ok(DirectEndpoint::Push),
        "/sync/v1/pull" => Ok(DirectEndpoint::Pull),
        "/sync/v1/checkpoint" => Ok(DirectEndpoint::Checkpoint),
        "/sync/v1/ack" => Ok(DirectEndpoint::Ack),
        _ => Err(MobileSyncRuntimeError::JournalCorrupt),
    }
}

const fn operation_name(endpoint: DirectEndpoint) -> &'static str {
    match endpoint {
        DirectEndpoint::Negotiate => "negotiate",
        DirectEndpoint::Bootstrap => "bootstrap",
        DirectEndpoint::Push => "push",
        DirectEndpoint::Pull => "pull",
        DirectEndpoint::Checkpoint => "checkpoint",
        DirectEndpoint::Ack => "ack",
    }
}

fn decode_sha256_hex(value: &str) -> Result<[u8; SHA256_BYTES], MobileSyncRuntimeError> {
    if value.len() != SHA256_BYTES * 2 {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    let mut bytes = [0_u8; SHA256_BYTES];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(chunk[0]).ok_or(MobileSyncRuntimeError::JournalCorrupt)?;
        let low = hex_nibble(chunk[1]).ok_or(MobileSyncRuntimeError::JournalCorrupt)?;
        bytes[index] = (high << 4) | low;
    }
    Ok(bytes)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn fixed_bytes<const N: usize>(bytes: &[u8]) -> Result<[u8; N], MobileSyncRuntimeError> {
    bytes
        .try_into()
        .map_err(|_| MobileSyncRuntimeError::JournalCorrupt)
}

fn positive_u64(value: i64) -> Result<u64, MobileSyncRuntimeError> {
    if value <= 0 {
        return Err(MobileSyncRuntimeError::JournalCorrupt);
    }
    u64::try_from(value).map_err(|_| MobileSyncRuntimeError::JournalCorrupt)
}

fn nonnegative_u64(value: i64) -> Result<u64, MobileSyncRuntimeError> {
    u64::try_from(value).map_err(|_| MobileSyncRuntimeError::JournalCorrupt)
}

fn store_error(_error: String) -> MobileSyncRuntimeError {
    MobileSyncRuntimeError::StateUnavailable
}
