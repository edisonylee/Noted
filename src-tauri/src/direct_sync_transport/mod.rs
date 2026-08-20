//! Fixture-only TLS 1.3 and HTTP/1.1 adapter for direct sync.
//!
//! This module deliberately has no Tauri command, application lifecycle hook,
//! Bonjour integration, or non-loopback bind option. It is a production-shaped
//! network checkpoint around the six typed operations in [`crate::direct_sync`]
//! while the production authority and cryptography work remains incomplete.
//! It is a native-client protocol, not a browser endpoint: requests carrying an
//! `Origin` header are rejected instead of participating in CORS.

mod client;
mod private_lan;

#[cfg(target_os = "macos")]
mod bonjour;

#[cfg(not(target_os = "ios"))]
mod server;

use crate::direct_sync::{
    DirectEndpoint, DirectRequest, DirectResponse, DirectSyncCrypto, DirectSyncEnrollment,
    DirectSyncLimits, DirectSyncService, EndpointLimits, ExactWireDirectSyncAuthority,
    DIRECT_TRANSACTION_REQUEST_BYTES, DIRECT_TRANSACTION_RESPONSE_BYTES, MAX_DIRECT_REQUEST_BYTES,
};
#[cfg(not(target_os = "ios"))]
use crate::pairing_protocol::TransportEvidence;
use std::fmt;

pub use client::FixtureDirectSyncClient;
pub use private_lan::{
    PrivateLanCandidateSource, PrivateLanDirectSyncSession, PrivateLanEndpointCandidate,
    PrivateLanSessionError,
};

#[cfg(all(target_os = "macos", feature = "sanitized-development-fixtures"))]
pub use bonjour::SanitizedBonjourAdvertisement;

#[cfg(all(not(target_os = "ios"), feature = "sanitized-development-fixtures"))]
pub use server::SanitizedPrivateLanServer;
#[cfg(not(target_os = "ios"))]
pub use server::{FixtureLoopbackServer, FixtureTlsIdentity};

pub(crate) const HTTP_1_1_ALPN: &[u8] = b"http/1.1";
pub(crate) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_HEADERS: usize = 24;
#[cfg(not(target_os = "ios"))]
pub(crate) const MAX_HEADER_BUFFER_BYTES: usize = 16 * 1024;
#[cfg(not(target_os = "ios"))]
pub(crate) const MAX_CONCURRENT_CONNECTIONS: usize = 8;

/// Failures exposed by the fixture transport intentionally omit peer-provided
/// strings and TLS details so callers cannot accidentally log sensitive wire
/// material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectSyncTransportError {
    InvalidFixtureConfiguration,
    LoopbackRequired,
    PrivateLanRequired,
    RequestTooLarge,
    ResponseTooLarge,
    InvalidHttpFraming,
    SecureTransportFailed,
    TimedOut,
    ConnectionFailed,
    ServerStopped,
}

impl fmt::Display for DirectSyncTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidFixtureConfiguration => "invalid fixture transport configuration",
            Self::LoopbackRequired => "fixture transport requires a loopback address",
            Self::PrivateLanRequired => "fixture transport requires a private IPv4 address",
            Self::RequestTooLarge => "direct-sync request exceeds its route limit",
            Self::ResponseTooLarge => "direct-sync response exceeds its route limit",
            Self::InvalidHttpFraming => "invalid direct-sync HTTP framing",
            Self::SecureTransportFailed => "direct-sync TLS authentication failed",
            Self::TimedOut => "direct-sync transport timed out",
            Self::ConnectionFailed => "direct-sync connection failed",
            Self::ServerStopped => "direct-sync fixture server stopped",
        })
    }
}

impl std::error::Error for DirectSyncTransportError {}

/// Immutable fixture policy shared by the client and loopback server.
///
/// There is intentionally no environment or data-class parameter: this
/// constructor can represent only the sanitized development fixture. The core
/// [`DirectSyncService`] independently enforces the same boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureTransportPolicy {
    expected_server_spki_sha256: [u8; 32],
    limits: DirectSyncLimits,
}

impl FixtureTransportPolicy {
    pub fn new_fixture_only(
        expected_server_spki_sha256: [u8; 32],
        limits: DirectSyncLimits,
    ) -> Result<Self, DirectSyncTransportError> {
        if expected_server_spki_sha256.iter().all(|byte| *byte == 0) || !valid_limits(&limits) {
            return Err(DirectSyncTransportError::InvalidFixtureConfiguration);
        }
        Ok(Self {
            expected_server_spki_sha256,
            limits,
        })
    }

    pub fn expected_server_spki_sha256(&self) -> [u8; 32] {
        self.expected_server_spki_sha256
    }

    pub fn limits(&self) -> &DirectSyncLimits {
        &self.limits
    }

    pub(crate) fn limits_for(&self, endpoint: DirectEndpoint) -> EndpointLimits {
        match endpoint {
            DirectEndpoint::Negotiate => self.limits.negotiate,
            DirectEndpoint::Bootstrap => self.limits.bootstrap,
            DirectEndpoint::Push => self.limits.push,
            DirectEndpoint::Pull => self.limits.pull,
            DirectEndpoint::Checkpoint => self.limits.checkpoint,
            DirectEndpoint::Ack => self.limits.ack,
        }
    }
}

/// The only backend seam available to the network server. It cannot dispatch
/// a Tauri command or select an arbitrary desktop operation.
pub trait DirectSyncRequestHandler: Send + Sync + 'static {
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse;
}

/// The only pairing routes the shared fixture authority listener can expose.
/// Keeping this closed enum beside the transport prevents the listener from
/// becoming a generic desktop command bridge.
#[cfg(not(target_os = "ios"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingEndpoint {
    ClientHello,
    Bootstrap,
    ClientFinish,
}

#[cfg(not(target_os = "ios"))]
impl PairingEndpoint {
    pub const CLIENT_HELLO_PATH: &'static str = "/pairing/v1/client-hello";
    pub const CLIENT_FINISH_PATH: &'static str = "/pairing/v1/client-finish";

    pub const fn path(self) -> &'static str {
        match self {
            Self::ClientHello => Self::CLIENT_HELLO_PATH,
            Self::Bootstrap => crate::direct_pairing_delivery::BOOTSTRAP_DELIVERY_ROUTE,
            Self::ClientFinish => Self::CLIENT_FINISH_PATH,
        }
    }
}

/// A pairing request whose TLS facts were derived by the native listener.
/// Protocol bodies and certificate pins never pass through JavaScript.
#[cfg(not(target_os = "ios"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTransportRequest {
    pub endpoint: PairingEndpoint,
    pub body: Vec<u8>,
    pub transport: TransportEvidence,
}

#[cfg(not(target_os = "ios"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTransportResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Narrow seam used only by the shared pairing-and-sync authority listener.
#[cfg(not(target_os = "ios"))]
pub trait FixtureAuthorityRequestHandler: DirectSyncRequestHandler {
    fn handle_pairing(&self, request: PairingTransportRequest) -> PairingTransportResponse;
}

impl<E, A, V> DirectSyncRequestHandler for DirectSyncService<E, A, V>
where
    E: DirectSyncEnrollment,
    A: ExactWireDirectSyncAuthority + 'static,
    V: DirectSyncCrypto,
{
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        self.handle(request)
    }
}

#[cfg(not(target_os = "ios"))]
pub(crate) fn endpoint_from_exact_target(target: &str) -> Option<DirectEndpoint> {
    DirectEndpoint::ALL
        .into_iter()
        .find(|endpoint| endpoint.path() == target)
}

#[cfg(not(target_os = "ios"))]
pub(crate) fn pairing_endpoint_from_exact_target(target: &str) -> Option<PairingEndpoint> {
    [
        PairingEndpoint::ClientHello,
        PairingEndpoint::Bootstrap,
        PairingEndpoint::ClientFinish,
    ]
    .into_iter()
    .find(|endpoint| endpoint.path() == target)
}

fn valid_limits(limits: &DirectSyncLimits) -> bool {
    let policy = FixtureTransportPolicy {
        expected_server_spki_sha256: [1; 32],
        limits: limits.clone(),
    };
    DirectEndpoint::ALL.into_iter().all(|endpoint| {
        let limit = policy.limits_for(endpoint);
        limit.request_bytes > 0
            && limit.request_bytes <= MAX_DIRECT_REQUEST_BYTES
            && limit.response_bytes > 0
            && limit.response_bytes <= MAX_RESPONSE_BYTES
    }) && limits.push.request_bytes >= DIRECT_TRANSACTION_REQUEST_BYTES
        && limits.pull.response_bytes >= DIRECT_TRANSACTION_RESPONSE_BYTES
}

#[cfg(test)]
mod tests;
