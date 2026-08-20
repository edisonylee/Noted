//! Native private-LAN client for the sanitized direct-sync fixture.
//!
//! The public boundary accepts only validated numeric socket addresses and an
//! already authenticated activation profile. Bonjour data is deliberately
//! reduced to an address hint: service names, TXT records, advertised pins,
//! hostnames, paths, and credentials have no representation here. The exact
//! P-256 SPKI pin always comes from the activation profile.

use super::client::{authority_header, build_client_config, collect_response};
use super::{DirectSyncTransportError, FixtureTransportPolicy, HTTP_1_1_ALPN};
use crate::direct_sync::{
    DirectEndpoint, DirectResponse, DirectSyncLimits, DIRECT_SYNC_CONTENT_TYPE,
};
use crate::mobile_sync_runtime::{
    ActiveSyncProfile, DirectSyncPostFuture, MobileSyncRuntimeError, VerifiedDirectSyncSession,
};
use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE};
use hyper::Request;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ProtocolVersion};
use std::collections::BTreeSet;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};
use tokio_rustls::{client::TlsStream, TlsConnector};

const MAX_ENDPOINT_CANDIDATES: usize = 16;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CANDIDATE_SELECTION_TIMEOUT: Duration = Duration::from_secs(15);
const HTTP_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const SMALL_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(12);
const BULK_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(30);

/// The native source that supplied an address hint. The source is diagnostic
/// provenance only and never changes authentication: both paths require the
/// same activation pin and pinned TLS handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateLanCandidateSource {
    BonjourAddressHint,
    ManualNumericAddress,
}

/// A syntactically and semantically bounded private-LAN address hint.
///
/// This type cannot contain a hostname, URL, route, query, credentials, or a
/// Bonjour-advertised certificate pin. IPv6 link-local addresses must include
/// a numeric interface scope, because connecting to an unscoped link-local
/// address is ambiguous on a multi-interface phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivateLanEndpointCandidate {
    address: SocketAddr,
    source: PrivateLanCandidateSource,
}

impl PrivateLanEndpointCandidate {
    /// Accept a numeric address emitted by the trusted native Bonjour adapter.
    /// All other Bonjour metadata is intentionally discarded before this call.
    pub fn from_bonjour_address_hint(address: SocketAddr) -> Result<Self, PrivateLanSessionError> {
        Self::validated(address, PrivateLanCandidateSource::BonjourAddressHint)
    }

    /// Parse strict numeric manual input such as `192.168.1.8:43123` or
    /// `[fd12::8]:43123`. Hostnames, schemes, paths, queries, userinfo, and
    /// missing ports do not parse as `SocketAddr` and therefore fail closed.
    pub fn parse_manual_numeric(input: &str) -> Result<Self, PrivateLanSessionError> {
        if input.is_empty() || input.trim() != input {
            return Err(PrivateLanSessionError::InvalidEndpointCandidate);
        }
        let address = input
            .parse::<SocketAddr>()
            .map_err(|_| PrivateLanSessionError::InvalidEndpointCandidate)?;
        Self::validated(address, PrivateLanCandidateSource::ManualNumericAddress)
    }

    pub fn address(self) -> SocketAddr {
        self.address
    }

    pub fn source(self) -> PrivateLanCandidateSource {
        self.source
    }

    fn validated(
        address: SocketAddr,
        source: PrivateLanCandidateSource,
    ) -> Result<Self, PrivateLanSessionError> {
        validate_private_lan_address(address)?;
        Ok(Self { address, source })
    }

    #[cfg(any(test, feature = "sanitized-development-fixtures"))]
    fn loopback_fixture(address: SocketAddr) -> Result<Self, PrivateLanSessionError> {
        if address.port() == 0 || !address.ip().is_loopback() {
            return Err(PrivateLanSessionError::InvalidEndpointCandidate);
        }
        Ok(Self {
            address,
            source: PrivateLanCandidateSource::ManualNumericAddress,
        })
    }
}

/// Errors intentionally omit peer-provided text, TLS details, addresses, and
/// response bodies so the native caller cannot accidentally log wire data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateLanSessionError {
    InvalidEndpointCandidate,
    NoEndpointCandidates,
    TooManyEndpointCandidates,
    InvalidAuthenticatedActivation,
    InvalidTransportConfiguration,
    RequestTooLarge,
    ResponseTooLarge,
    InvalidHttpFraming,
    SecureTransportFailed,
    TimedOut,
    ConnectionFailed,
}

impl fmt::Display for PrivateLanSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEndpointCandidate => "invalid private-LAN endpoint candidate",
            Self::NoEndpointCandidates => "no private-LAN endpoint candidates",
            Self::TooManyEndpointCandidates => "too many private-LAN endpoint candidates",
            Self::InvalidAuthenticatedActivation => "authenticated activation is invalid",
            Self::InvalidTransportConfiguration => "private-LAN transport configuration is invalid",
            Self::RequestTooLarge => "direct-sync request exceeds its endpoint limit",
            Self::ResponseTooLarge => "direct-sync response exceeds its endpoint limit",
            Self::InvalidHttpFraming => "invalid direct-sync HTTP framing",
            Self::SecureTransportFailed => "direct-sync TLS authentication failed",
            Self::TimedOut => "direct-sync transport timed out",
            Self::ConnectionFailed => "direct-sync endpoint is unavailable",
        })
    }
}

impl std::error::Error for PrivateLanSessionError {}

impl From<DirectSyncTransportError> for PrivateLanSessionError {
    fn from(error: DirectSyncTransportError) -> Self {
        match error {
            DirectSyncTransportError::InvalidFixtureConfiguration
            | DirectSyncTransportError::LoopbackRequired => Self::InvalidTransportConfiguration,
            DirectSyncTransportError::RequestTooLarge => Self::RequestTooLarge,
            DirectSyncTransportError::ResponseTooLarge => Self::ResponseTooLarge,
            DirectSyncTransportError::InvalidHttpFraming => Self::InvalidHttpFraming,
            DirectSyncTransportError::SecureTransportFailed => Self::SecureTransportFailed,
            DirectSyncTransportError::TimedOut => Self::TimedOut,
            DirectSyncTransportError::ConnectionFailed
            | DirectSyncTransportError::ServerStopped => Self::ConnectionFailed,
        }
    }
}

/// A native-only, pinned private-LAN direct-sync session.
///
/// Each request uses a fresh TCP/TLS/HTTP connection. Candidate fallback may
/// happen only before a pinned TLS handshake succeeds, so the implementation
/// never silently replays application bytes after a partial HTTP exchange.
#[derive(Clone)]
pub struct PrivateLanDirectSyncSession {
    candidates: Arc<[PrivateLanEndpointCandidate]>,
    policy: FixtureTransportPolicy,
    tls_config: Arc<ClientConfig>,
    last_authenticated_address: Arc<Mutex<Option<SocketAddr>>>,
}

impl PrivateLanDirectSyncSession {
    /// Construct a session from the durable, authenticated activation profile.
    /// The caller cannot supply or override a pin through discovery metadata.
    pub fn from_authenticated_activation(
        profile: &ActiveSyncProfile,
        candidates: impl IntoIterator<Item = PrivateLanEndpointCandidate>,
        limits: DirectSyncLimits,
    ) -> Result<Self, PrivateLanSessionError> {
        profile
            .validate_fixture()
            .map_err(|_| PrivateLanSessionError::InvalidAuthenticatedActivation)?;
        let candidates = validate_candidate_set(candidates)?;
        let policy =
            FixtureTransportPolicy::new_fixture_only(profile.durable_sync_spki_sha256, limits)?;
        let tls_config = Arc::new(build_client_config(&policy)?);
        Ok(Self {
            candidates: candidates.into(),
            policy,
            tls_config,
            last_authenticated_address: Arc::new(Mutex::new(None)),
        })
    }

    /// Send one exact body to one of the six typed direct-sync endpoints.
    /// There is no retry after application bytes are submitted.
    pub async fn post_exact(
        &self,
        endpoint: DirectEndpoint,
        body: Vec<u8>,
    ) -> Result<DirectResponse, PrivateLanSessionError> {
        let limits = self.policy.limits_for(endpoint);
        if body.is_empty() {
            return Err(PrivateLanSessionError::InvalidHttpFraming);
        }
        if body.len() > limits.request_bytes {
            return Err(PrivateLanSessionError::RequestTooLarge);
        }

        let (address, tls) = timeout(CANDIDATE_SELECTION_TIMEOUT, self.connect_authenticated())
            .await
            .map_err(|_| PrivateLanSessionError::TimedOut)??;
        self.remember_authenticated_address(address);

        let io = TokioIo::new(tls);
        let (mut sender, connection) = timeout(
            HTTP_HANDSHAKE_TIMEOUT,
            hyper::client::conn::http1::handshake(io),
        )
        .await
        .map_err(|_| PrivateLanSessionError::TimedOut)?
        .map_err(|_| PrivateLanSessionError::ConnectionFailed)?;
        let _connection_guard = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));

        let request = Request::builder()
            .method("POST")
            .uri(endpoint.path())
            .header("host", authority_header(address))
            .header(CONTENT_TYPE, DIRECT_SYNC_CONTENT_TYPE)
            .header(CONTENT_LENGTH, body.len().to_string())
            .header(CONNECTION, "close")
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| PrivateLanSessionError::InvalidHttpFraming)?;

        let deadline = Instant::now() + endpoint_timeout(endpoint);
        let response = timeout(
            deadline.saturating_duration_since(Instant::now()),
            sender.send_request(request),
        )
        .await
        .map_err(|_| PrivateLanSessionError::TimedOut)?
        .map_err(|_| PrivateLanSessionError::ConnectionFailed)?;
        drop(sender);

        timeout(
            deadline.saturating_duration_since(Instant::now()),
            collect_response(response, limits.response_bytes),
        )
        .await
        .map_err(|_| PrivateLanSessionError::TimedOut)?
        .map_err(PrivateLanSessionError::from)
    }

    async fn connect_authenticated(
        &self,
    ) -> Result<(SocketAddr, TlsStream<TcpStream>), PrivateLanSessionError> {
        let mut saw_tcp_connection = false;
        let mut saw_tls_failure = false;
        for address in self.ordered_addresses() {
            let tcp = match timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
                Ok(Ok(tcp)) => {
                    saw_tcp_connection = true;
                    tcp
                }
                Ok(Err(_)) | Err(_) => continue,
            };
            let server_name = ServerName::IpAddress(address.ip().into());
            let tls = match timeout(
                TLS_HANDSHAKE_TIMEOUT,
                TlsConnector::from(Arc::clone(&self.tls_config)).connect(server_name, tcp),
            )
            .await
            {
                Ok(Ok(tls)) => tls,
                Ok(Err(_)) => {
                    saw_tls_failure = true;
                    continue;
                }
                Err(_) => continue,
            };
            let connection = tls.get_ref().1;
            if connection.protocol_version() != Some(ProtocolVersion::TLSv1_3)
                || connection.alpn_protocol() != Some(HTTP_1_1_ALPN)
                || connection.is_early_data_accepted()
            {
                saw_tls_failure = true;
                continue;
            }
            return Ok((address, tls));
        }

        if saw_tls_failure || saw_tcp_connection {
            Err(PrivateLanSessionError::SecureTransportFailed)
        } else {
            Err(PrivateLanSessionError::ConnectionFailed)
        }
    }

    fn ordered_addresses(&self) -> Vec<SocketAddr> {
        let remembered = self
            .last_authenticated_address
            .lock()
            .ok()
            .and_then(|guard| *guard);
        let mut ordered = Vec::with_capacity(self.candidates.len());
        if let Some(address) = remembered {
            ordered.push(address);
        }
        ordered.extend(
            self.candidates
                .iter()
                .map(|candidate| candidate.address)
                .filter(|address| Some(*address) != remembered),
        );
        ordered
    }

    fn remember_authenticated_address(&self, address: SocketAddr) {
        if let Ok(mut remembered) = self.last_authenticated_address.lock() {
            *remembered = Some(address);
        }
    }

    /// Construct the production pinned-TLS session against the loopback-only
    /// fixture authority. This escape hatch is absent from ordinary builds.
    #[doc(hidden)]
    #[cfg(any(test, feature = "sanitized-development-fixtures"))]
    pub fn from_loopback_fixture_for_test(
        profile: &ActiveSyncProfile,
        address: SocketAddr,
        limits: DirectSyncLimits,
    ) -> Result<Self, PrivateLanSessionError> {
        let candidate = PrivateLanEndpointCandidate::loopback_fixture(address)?;
        Self::from_fixture_candidates_for_test(profile, vec![candidate], limits)
    }

    #[cfg(any(test, feature = "sanitized-development-fixtures"))]
    fn from_fixture_candidates_for_test(
        profile: &ActiveSyncProfile,
        candidates: Vec<PrivateLanEndpointCandidate>,
        limits: DirectSyncLimits,
    ) -> Result<Self, PrivateLanSessionError> {
        if candidates.is_empty() || candidates.len() > MAX_ENDPOINT_CANDIDATES {
            return Err(PrivateLanSessionError::InvalidEndpointCandidate);
        }
        profile
            .validate_fixture()
            .map_err(|_| PrivateLanSessionError::InvalidAuthenticatedActivation)?;
        let policy =
            FixtureTransportPolicy::new_fixture_only(profile.durable_sync_spki_sha256, limits)?;
        let tls_config = Arc::new(build_client_config(&policy)?);
        Ok(Self {
            candidates: candidates.into(),
            policy,
            tls_config,
            last_authenticated_address: Arc::new(Mutex::new(None)),
        })
    }
}

impl VerifiedDirectSyncSession for PrivateLanDirectSyncSession {
    fn post<'a>(
        &'a self,
        endpoint: DirectEndpoint,
        exact_body: Vec<u8>,
    ) -> DirectSyncPostFuture<'a> {
        Box::pin(async move {
            self.post_exact(endpoint, exact_body)
                .await
                .map_err(runtime_error)
        })
    }
}

fn validate_candidate_set(
    candidates: impl IntoIterator<Item = PrivateLanEndpointCandidate>,
) -> Result<Vec<PrivateLanEndpointCandidate>, PrivateLanSessionError> {
    let mut addresses = BTreeSet::new();
    let mut validated = Vec::new();
    for candidate in candidates {
        validate_private_lan_address(candidate.address)?;
        if addresses.insert(candidate.address) {
            if validated.len() == MAX_ENDPOINT_CANDIDATES {
                return Err(PrivateLanSessionError::TooManyEndpointCandidates);
            }
            validated.push(candidate);
        }
    }
    if validated.is_empty() {
        return Err(PrivateLanSessionError::NoEndpointCandidates);
    }
    Ok(validated)
}

fn validate_private_lan_address(address: SocketAddr) -> Result<(), PrivateLanSessionError> {
    if address.port() == 0 {
        return Err(PrivateLanSessionError::InvalidEndpointCandidate);
    }
    let accepted = match address {
        SocketAddr::V4(address) => is_rfc1918(*address.ip()) || address.ip().is_link_local(),
        SocketAddr::V6(address) => {
            let ip = *address.ip();
            let is_link_local = is_ipv6_link_local(ip);
            let valid_scope = if is_link_local {
                address.scope_id() != 0
            } else {
                address.scope_id() == 0
            };
            (is_ipv6_unique_local(ip) || is_link_local) && valid_scope && address.flowinfo() == 0
        }
    };
    if !accepted || address.ip().is_unspecified() || address.ip().is_multicast() {
        return Err(PrivateLanSessionError::InvalidEndpointCandidate);
    }
    Ok(())
}

fn is_rfc1918(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

fn is_ipv6_unique_local(address: Ipv6Addr) -> bool {
    address.octets()[0] & 0xfe == 0xfc
}

fn is_ipv6_link_local(address: Ipv6Addr) -> bool {
    let octets = address.octets();
    octets[0] == 0xfe && octets[1] & 0xc0 == 0x80
}

fn endpoint_timeout(endpoint: DirectEndpoint) -> Duration {
    match endpoint {
        DirectEndpoint::Bootstrap | DirectEndpoint::Push | DirectEndpoint::Pull => {
            BULK_ENDPOINT_TIMEOUT
        }
        DirectEndpoint::Negotiate | DirectEndpoint::Checkpoint | DirectEndpoint::Ack => {
            SMALL_ENDPOINT_TIMEOUT
        }
    }
}

fn runtime_error(error: PrivateLanSessionError) -> MobileSyncRuntimeError {
    match error {
        PrivateLanSessionError::RequestTooLarge => MobileSyncRuntimeError::RequestTooLarge,
        PrivateLanSessionError::ResponseTooLarge | PrivateLanSessionError::InvalidHttpFraming => {
            MobileSyncRuntimeError::InvalidResponse
        }
        PrivateLanSessionError::InvalidEndpointCandidate
        | PrivateLanSessionError::NoEndpointCandidates
        | PrivateLanSessionError::TooManyEndpointCandidates
        | PrivateLanSessionError::InvalidAuthenticatedActivation
        | PrivateLanSessionError::InvalidTransportConfiguration
        | PrivateLanSessionError::SecureTransportFailed
        | PrivateLanSessionError::TimedOut
        | PrivateLanSessionError::ConnectionFailed => MobileSyncRuntimeError::TransportUnavailable,
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct_sync::{DirectRequest, DirectSyncLimits};
    use crate::direct_sync_transport::{
        DirectSyncRequestHandler, FixtureLoopbackServer, FixtureTlsIdentity,
    };
    use crate::pairing_protocol::{Environment, KindCapability, LibraryDataClass, RecordKind};
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::{Ipv4Addr, SocketAddrV4, SocketAddrV6};

    struct EchoHandler;

    impl DirectSyncRequestHandler for EchoHandler {
        fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
            DirectResponse {
                status: 202,
                content_type: DIRECT_SYNC_CONTENT_TYPE,
                body: request.body,
            }
        }
    }

    fn profile(pin: [u8; 32]) -> ActiveSyncProfile {
        let granted_scopes: BTreeSet<_> =
            [RecordKind::Note, RecordKind::Category, RecordKind::Folder]
                .into_iter()
                .collect();
        let capabilities: BTreeMap<_, _> = granted_scopes
            .iter()
            .copied()
            .map(|kind| {
                (
                    kind,
                    KindCapability {
                        reader_version: 1,
                        writer_version: Some(1),
                    },
                )
            })
            .collect();
        let mut public_key = vec![0x04];
        public_key.extend([0x33; 64]);
        ActiveSyncProfile {
            identity_handle: "018f47f2-8ee8-4a28-91eb-9b3f2619e071".to_owned(),
            receipt_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e075".to_owned(),
            activation_sha256: "a".repeat(64),
            library_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e072".to_owned(),
            device_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e073".to_owned(),
            default_scope_id: "018f47f2-8ee8-7a28-91eb-9b3f2619e074".to_owned(),
            authority_generation: 1,
            purge_generation: 0,
            key_epoch: 1,
            environment: Environment::Development,
            library_data_class: LibraryDataClass::SanitizedFixture,
            durable_sync_spki_sha256: pin,
            device_signing_public_key: public_key.clone(),
            authority_signing_public_key: public_key,
            granted_scopes,
            capabilities,
            revoked: false,
        }
    }

    async fn fixture() -> (FixtureLoopbackServer, PrivateLanDirectSyncSession, [u8; 32]) {
        let identity = FixtureTlsIdentity::generate().unwrap();
        let pin = identity.spki_sha256();
        let limits = DirectSyncLimits::default();
        let policy = FixtureTransportPolicy::new_fixture_only(pin, limits.clone()).unwrap();
        let server =
            FixtureLoopbackServer::spawn_fixture_only(Arc::new(EchoHandler), identity, policy)
                .await
                .unwrap();
        let session = PrivateLanDirectSyncSession::from_loopback_fixture_for_test(
            &profile(pin),
            server.local_addr(),
            limits,
        )
        .unwrap();
        (server, session, pin)
    }

    #[test]
    fn accepts_only_numeric_private_or_link_local_unicast_candidates() {
        let accepted = [
            "10.0.0.8:43123",
            "172.16.2.4:43123",
            "172.31.255.254:43123",
            "192.168.50.2:43123",
            "169.254.10.4:43123",
            "[fd12:3456::8]:43123",
            "[fe80::8%4]:43123",
        ];
        for address in accepted {
            assert!(
                PrivateLanEndpointCandidate::parse_manual_numeric(address).is_ok(),
                "{address}"
            );
        }

        let rejected = [
            "noted.local:43123",
            "https://192.168.1.8:43123/sync/v1/pull",
            "user@192.168.1.8:43123",
            "192.168.1.8:43123/sync",
            "192.168.1.8:43123?token=secret",
            "192.168.1.8",
            " 192.168.1.8:43123",
            "8.8.8.8:43123",
            "0.0.0.0:43123",
            "224.0.0.251:43123",
            "127.0.0.1:43123",
            "192.168.1.8:0",
            "[::]:43123",
            "[ff02::fb%4]:43123",
            "[2001:4860:4860::8888]:43123",
            "[fe80::8]:43123",
            "[fd12:3456::8%4]:43123",
        ];
        for address in rejected {
            assert_eq!(
                PrivateLanEndpointCandidate::parse_manual_numeric(address),
                Err(PrivateLanSessionError::InvalidEndpointCandidate),
                "{address}"
            );
        }
    }

    #[test]
    fn bonjour_path_accepts_only_an_address_hint_and_deduplicates_candidates() {
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 8), 43123));
        let bonjour = PrivateLanEndpointCandidate::from_bonjour_address_hint(address).unwrap();
        assert_eq!(bonjour.address(), address);
        assert_eq!(
            bonjour.source(),
            PrivateLanCandidateSource::BonjourAddressHint
        );

        let session = PrivateLanDirectSyncSession::from_authenticated_activation(
            &profile([9; 32]),
            [bonjour, bonjour],
            DirectSyncLimits::default(),
        )
        .unwrap();
        assert_eq!(session.candidates.len(), 1);
    }

    #[test]
    fn candidate_set_and_activation_are_bounded() {
        assert!(matches!(
            PrivateLanDirectSyncSession::from_authenticated_activation(
                &profile([9; 32]),
                [],
                DirectSyncLimits::default(),
            ),
            Err(PrivateLanSessionError::NoEndpointCandidates)
        ));

        let candidates = (1..=MAX_ENDPOINT_CANDIDATES + 1).map(|last| {
            PrivateLanEndpointCandidate::from_bonjour_address_hint(SocketAddr::V4(
                SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, last as u8), 43123),
            ))
            .unwrap()
        });
        assert!(matches!(
            PrivateLanDirectSyncSession::from_authenticated_activation(
                &profile([9; 32]),
                candidates,
                DirectSyncLimits::default(),
            ),
            Err(PrivateLanSessionError::TooManyEndpointCandidates)
        ));

        let mut revoked = profile([9; 32]);
        revoked.revoked = true;
        let candidate =
            PrivateLanEndpointCandidate::parse_manual_numeric("10.0.0.8:43123").unwrap();
        assert!(matches!(
            PrivateLanDirectSyncSession::from_authenticated_activation(
                &revoked,
                [candidate],
                DirectSyncLimits::default(),
            ),
            Err(PrivateLanSessionError::InvalidAuthenticatedActivation)
        ));

        let missing_pin = profile([0; 32]);
        assert!(matches!(
            PrivateLanDirectSyncSession::from_authenticated_activation(
                &missing_pin,
                [candidate],
                DirectSyncLimits::default(),
            ),
            Err(PrivateLanSessionError::InvalidAuthenticatedActivation)
        ));
    }

    #[test]
    fn socket_constructor_rejects_public_multicast_unspecified_and_zero_port() {
        let rejected = [
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 443)),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 443)),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(239, 1, 2, 3), 443)),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 1, 2, 3), 0)),
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 443, 0, 0)),
            SocketAddr::V6(SocketAddrV6::new("ff02::fb".parse().unwrap(), 443, 0, 4)),
        ];
        for address in rejected {
            assert_eq!(
                PrivateLanEndpointCandidate::from_bonjour_address_hint(address),
                Err(PrivateLanSessionError::InvalidEndpointCandidate)
            );
        }
    }

    #[tokio::test]
    async fn typed_session_round_trips_all_six_routes_and_preserves_response() {
        let (server, session, _pin) = fixture().await;
        for endpoint in DirectEndpoint::ALL {
            let body = format!("{{\"endpoint\":\"{}\"}}", endpoint.path()).into_bytes();
            let response = session.post_exact(endpoint, body.clone()).await.unwrap();
            assert_eq!(response.status, 202);
            assert_eq!(response.content_type, DIRECT_SYNC_CONTENT_TYPE);
            assert_eq!(response.body, body);
        }
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn wrong_activation_pin_fails_before_http_request() {
        let identity = FixtureTlsIdentity::generate().unwrap();
        let server_pin = identity.spki_sha256();
        let limits = DirectSyncLimits::default();
        let policy = FixtureTransportPolicy::new_fixture_only(server_pin, limits.clone()).unwrap();
        let server =
            FixtureLoopbackServer::spawn_fixture_only(Arc::new(EchoHandler), identity, policy)
                .await
                .unwrap();
        let session = PrivateLanDirectSyncSession::from_loopback_fixture_for_test(
            &profile([0x77; 32]),
            server.local_addr(),
            limits,
        )
        .unwrap();
        assert_eq!(
            session
                .post_exact(DirectEndpoint::Negotiate, br#"{}"#.to_vec())
                .await,
            Err(PrivateLanSessionError::SecureTransportFailed)
        );
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn selection_skips_unavailable_hints_then_remembers_the_authenticated_address() {
        let identity = FixtureTlsIdentity::generate().unwrap();
        let pin = identity.spki_sha256();
        let limits = DirectSyncLimits::default();
        let policy = FixtureTransportPolicy::new_fixture_only(pin, limits.clone()).unwrap();
        let server =
            FixtureLoopbackServer::spawn_fixture_only(Arc::new(EchoHandler), identity, policy)
                .await
                .unwrap();
        let unavailable =
            PrivateLanEndpointCandidate::loopback_fixture("127.0.0.1:9".parse().unwrap()).unwrap();
        let available = PrivateLanEndpointCandidate::loopback_fixture(server.local_addr()).unwrap();
        let session = PrivateLanDirectSyncSession::from_fixture_candidates_for_test(
            &profile(pin),
            vec![unavailable, available],
            limits,
        )
        .unwrap();

        let response = session
            .post_exact(DirectEndpoint::Checkpoint, br#"{}"#.to_vec())
            .await
            .unwrap();
        assert_eq!(response.status, 202);
        assert_eq!(session.ordered_addresses()[0], server.local_addr());
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn trait_maps_network_failures_without_exposing_peer_details() {
        let session = PrivateLanDirectSyncSession::from_loopback_fixture_for_test(
            &profile([9; 32]),
            "127.0.0.1:9".parse().unwrap(),
            DirectSyncLimits::default(),
        )
        .unwrap();
        assert_eq!(
            VerifiedDirectSyncSession::post(
                &session,
                DirectEndpoint::Checkpoint,
                br#"{}"#.to_vec(),
            )
            .await,
            Err(MobileSyncRuntimeError::TransportUnavailable)
        );
    }

    #[tokio::test]
    async fn endpoint_request_limit_is_checked_before_any_connection_attempt() {
        let candidate =
            PrivateLanEndpointCandidate::parse_manual_numeric("10.0.0.8:43123").unwrap();
        let limits = DirectSyncLimits::default();
        let oversized = vec![b'x'; limits.negotiate.request_bytes + 1];
        let session = PrivateLanDirectSyncSession::from_authenticated_activation(
            &profile([9; 32]),
            [candidate],
            limits,
        )
        .unwrap();
        assert_eq!(
            session
                .post_exact(DirectEndpoint::Negotiate, oversized)
                .await,
            Err(PrivateLanSessionError::RequestTooLarge)
        );
    }

    #[test]
    fn endpoint_timeouts_are_route_specific_and_bounded() {
        for endpoint in [
            DirectEndpoint::Negotiate,
            DirectEndpoint::Checkpoint,
            DirectEndpoint::Ack,
        ] {
            assert_eq!(endpoint_timeout(endpoint), SMALL_ENDPOINT_TIMEOUT);
        }
        for endpoint in [
            DirectEndpoint::Bootstrap,
            DirectEndpoint::Push,
            DirectEndpoint::Pull,
        ] {
            assert_eq!(endpoint_timeout(endpoint), BULK_ENDPOINT_TIMEOUT);
        }
        assert!(BULK_ENDPOINT_TIMEOUT <= Duration::from_secs(30));
    }
}
