use super::client::exact_content_length;
use super::{
    endpoint_from_exact_target, pairing_endpoint_from_exact_target, DirectSyncRequestHandler,
    DirectSyncTransportError, FixtureAuthorityRequestHandler, FixtureTransportPolicy,
    PairingEndpoint, PairingTransportRequest, PairingTransportResponse, HTTP_1_1_ALPN,
    MAX_CONCURRENT_CONNECTIONS, MAX_HEADERS, MAX_HEADER_BUFFER_BYTES,
};
use crate::direct_pairing_delivery::{
    MAX_BOOTSTRAP_POLL_REQUEST_BYTES, MAX_BOOTSTRAP_POLL_RESPONSE_BYTES,
};
use crate::direct_sync::{
    DirectEndpoint, DirectRequest, DirectResponse, EndpointLimits, SecureTransportEvidence,
    DIRECT_SYNC_CONTENT_TYPE,
};
use crate::pairing_protocol::{TransportEvidence, MAX_PAIRING_MESSAGE_BYTES};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE,
    EXPECT, HOST, ORIGIN, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioIo, TokioTimer};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::NoServerSessionStorage;
use rustls::{ProtocolVersion, ServerConfig};
use sha2::{Digest, Sha256};
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const BODY_TIMEOUT: Duration = Duration::from_secs(30);
const HANDLER_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireEndpoint {
    Direct(DirectEndpoint),
    Pairing(PairingEndpoint),
}

impl WireEndpoint {
    fn limits(self, policy: &FixtureTransportPolicy) -> EndpointLimits {
        match self {
            Self::Direct(endpoint) => policy.limits_for(endpoint),
            Self::Pairing(PairingEndpoint::Bootstrap) => EndpointLimits {
                request_bytes: MAX_BOOTSTRAP_POLL_REQUEST_BYTES,
                response_bytes: MAX_BOOTSTRAP_POLL_RESPONSE_BYTES,
            },
            Self::Pairing(_) => EndpointLimits {
                request_bytes: MAX_PAIRING_MESSAGE_BYTES,
                response_bytes: MAX_PAIRING_MESSAGE_BYTES,
            },
        }
    }
}

fn wire_endpoint_from_exact_target(target: &str, pairing_enabled: bool) -> Option<WireEndpoint> {
    endpoint_from_exact_target(target)
        .map(WireEndpoint::Direct)
        .or_else(|| {
            pairing_enabled
                .then(|| pairing_endpoint_from_exact_target(target))
                .flatten()
                .map(WireEndpoint::Pairing)
        })
}

/// An in-memory P-256 certificate and key generated only for the sanitized
/// loopback fixture. It is neither persisted nor accepted by production APIs.
pub struct FixtureTlsIdentity {
    certificate: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
    spki_sha256: [u8; 32],
}

impl FixtureTlsIdentity {
    pub fn generate() -> Result<Self, DirectSyncTransportError> {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?;
        let spki_sha256 = Sha256::digest(key_pair.public_key_der()).into();
        let certificate = CertificateParams::new(vec![
            "localhost".to_owned(),
            Ipv4Addr::LOCALHOST.to_string(),
        ])
        .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?
        .self_signed(&key_pair)
        .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?;
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        Ok(Self {
            certificate: certificate.der().clone(),
            private_key,
            spki_sha256,
        })
    }

    /// Generate the pinned fixture identity for a private-LAN listener. The
    /// certificate is still ephemeral and can protect sanitized data only.
    #[cfg(feature = "sanitized-development-fixtures")]
    pub fn generate_for_private_lan(address: Ipv4Addr) -> Result<Self, DirectSyncTransportError> {
        if !is_private_lan_ipv4(address) {
            return Err(DirectSyncTransportError::PrivateLanRequired);
        }
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?;
        let spki_sha256 = Sha256::digest(key_pair.public_key_der()).into();
        let certificate = CertificateParams::new(vec![address.to_string()])
            .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?
            .self_signed(&key_pair)
            .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?;
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        Ok(Self {
            certificate: certificate.der().clone(),
            private_key,
            spki_sha256,
        })
    }

    pub fn spki_sha256(&self) -> [u8; 32] {
        self.spki_sha256
    }

    #[cfg(test)]
    pub(crate) fn certificate_der_for_test(&self) -> &[u8] {
        self.certificate.as_ref()
    }
}

trait WireRequestHandler: Send + Sync + 'static {
    fn pairing_enabled(&self) -> bool;
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse;
    fn handle_pairing(&self, request: PairingTransportRequest) -> PairingTransportResponse;
}

struct DirectOnlyHandler<H>(Arc<H>);

impl<H> WireRequestHandler for DirectOnlyHandler<H>
where
    H: DirectSyncRequestHandler,
{
    fn pairing_enabled(&self) -> bool {
        false
    }

    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        self.0.handle_direct_sync(request)
    }

    fn handle_pairing(&self, _request: PairingTransportRequest) -> PairingTransportResponse {
        PairingTransportResponse {
            status: StatusCode::NOT_FOUND.as_u16(),
            body: br#"{"error":{"code":"route_not_found"}}"#.to_vec(),
        }
    }
}

struct SharedAuthorityHandler<H>(Arc<H>);

impl<H> WireRequestHandler for SharedAuthorityHandler<H>
where
    H: FixtureAuthorityRequestHandler,
{
    fn pairing_enabled(&self) -> bool {
        true
    }

    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        self.0.handle_direct_sync(request)
    }

    fn handle_pairing(&self, request: PairingTransportRequest) -> PairingTransportResponse {
        self.0.handle_pairing(request)
    }
}

/// Owned loopback server handle. Dropping it signals shutdown; calling
/// [`shutdown`](Self::shutdown) also waits for accepted fixture connections to
/// finish their bounded TLS/HTTP work.
pub struct FixtureLoopbackServer {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    accept_task: Option<JoinHandle<()>>,
}

impl FixtureLoopbackServer {
    pub async fn spawn_fixture_only<H>(
        handler: Arc<H>,
        identity: FixtureTlsIdentity,
        policy: FixtureTransportPolicy,
    ) -> Result<Self, DirectSyncTransportError>
    where
        H: DirectSyncRequestHandler,
    {
        if identity.spki_sha256 != policy.expected_server_spki_sha256() {
            return Err(DirectSyncTransportError::InvalidFixtureConfiguration);
        }
        let tls_config = Arc::new(build_server_config(identity)?);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        let local_addr = listener
            .local_addr()
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        if !local_addr.ip().is_loopback() || local_addr.port() == 0 {
            return Err(DirectSyncTransportError::LoopbackRequired);
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handler = Arc::new(DirectOnlyHandler(handler));
        let accept_task = tokio::spawn(run_accept_loop(
            listener,
            local_addr,
            TlsAcceptor::from(tls_config),
            handler,
            policy,
            PeerBoundary::Loopback,
            shutdown_rx,
        ));
        Ok(Self {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
            accept_task: Some(accept_task),
        })
    }

    #[cfg(test)]
    pub(crate) async fn spawn_authority_fixture_only<H>(
        handler: Arc<H>,
        identity: FixtureTlsIdentity,
        policy: FixtureTransportPolicy,
    ) -> Result<Self, DirectSyncTransportError>
    where
        H: FixtureAuthorityRequestHandler,
    {
        if identity.spki_sha256 != policy.expected_server_spki_sha256() {
            return Err(DirectSyncTransportError::InvalidFixtureConfiguration);
        }
        let tls_config = Arc::new(build_server_config(identity)?);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        let local_addr = listener
            .local_addr()
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handler = Arc::new(SharedAuthorityHandler(handler));
        let accept_task = tokio::spawn(run_accept_loop(
            listener,
            local_addr,
            TlsAcceptor::from(tls_config),
            handler,
            policy,
            PeerBoundary::Loopback,
            shutdown_rx,
        ));
        Ok(Self {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
            accept_task: Some(accept_task),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(mut self) -> Result<(), DirectSyncTransportError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(accept_task) = self.accept_task.take() {
            accept_task
                .await
                .map_err(|_| DirectSyncTransportError::ServerStopped)?;
        }
        Ok(())
    }
}

/// Sanitized-fixture-only private-LAN listener. This is deliberately a
/// separate type from the loopback harness so an ordinary build cannot widen
/// its bind scope accidentally.
#[cfg(feature = "sanitized-development-fixtures")]
pub struct SanitizedPrivateLanServer {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    accept_task: Option<JoinHandle<()>>,
}

#[cfg(feature = "sanitized-development-fixtures")]
impl SanitizedPrivateLanServer {
    pub async fn spawn_fixture_only<H>(
        address: Ipv4Addr,
        handler: Arc<H>,
        identity: FixtureTlsIdentity,
        policy: FixtureTransportPolicy,
    ) -> Result<Self, DirectSyncTransportError>
    where
        H: DirectSyncRequestHandler,
    {
        if !is_private_lan_ipv4(address) {
            return Err(DirectSyncTransportError::PrivateLanRequired);
        }
        if identity.spki_sha256 != policy.expected_server_spki_sha256() {
            return Err(DirectSyncTransportError::InvalidFixtureConfiguration);
        }
        Self::spawn(
            address,
            Arc::new(DirectOnlyHandler(handler)),
            identity,
            policy,
        )
        .await
    }

    /// Start the single fixture authority listener that owns both the three
    /// fixed pairing routes and the six fixed direct-sync routes.
    pub async fn spawn_authority_fixture_only<H>(
        address: Ipv4Addr,
        handler: Arc<H>,
        identity: FixtureTlsIdentity,
        policy: FixtureTransportPolicy,
    ) -> Result<Self, DirectSyncTransportError>
    where
        H: FixtureAuthorityRequestHandler,
    {
        Self::spawn(
            address,
            Arc::new(SharedAuthorityHandler(handler)),
            identity,
            policy,
        )
        .await
    }

    async fn spawn<H>(
        address: Ipv4Addr,
        handler: Arc<H>,
        identity: FixtureTlsIdentity,
        policy: FixtureTransportPolicy,
    ) -> Result<Self, DirectSyncTransportError>
    where
        H: WireRequestHandler,
    {
        if !is_private_lan_ipv4(address) {
            return Err(DirectSyncTransportError::PrivateLanRequired);
        }
        if identity.spki_sha256 != policy.expected_server_spki_sha256() {
            return Err(DirectSyncTransportError::InvalidFixtureConfiguration);
        }
        let tls_config = Arc::new(build_server_config(identity)?);
        let listener = TcpListener::bind((address, 0))
            .await
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        let local_addr = listener
            .local_addr()
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        if local_addr.ip() != IpAddr::V4(address) || local_addr.port() == 0 {
            return Err(DirectSyncTransportError::PrivateLanRequired);
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let accept_task = tokio::spawn(run_accept_loop(
            listener,
            local_addr,
            TlsAcceptor::from(tls_config),
            handler,
            policy,
            PeerBoundary::PrivateLan,
            shutdown_rx,
        ));
        Ok(Self {
            local_addr,
            shutdown_tx: Some(shutdown_tx),
            accept_task: Some(accept_task),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(mut self) -> Result<(), DirectSyncTransportError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(accept_task) = self.accept_task.take() {
            accept_task
                .await
                .map_err(|_| DirectSyncTransportError::ServerStopped)?;
        }
        Ok(())
    }
}

#[cfg(feature = "sanitized-development-fixtures")]
impl Drop for SanitizedPrivateLanServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PeerBoundary {
    Loopback,
    PrivateLan,
}

impl PeerBoundary {
    fn accepts(self, address: IpAddr) -> bool {
        match (self, address) {
            (Self::Loopback, address) => address.is_loopback(),
            (Self::PrivateLan, IpAddr::V4(address)) => is_private_lan_ipv4(address),
            (Self::PrivateLan, IpAddr::V6(_)) => false,
        }
    }
}

pub(super) fn is_private_lan_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && (octets[0] == 10
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            || (octets[0] == 192 && octets[1] == 168)
            || (octets[0] == 169 && octets[1] == 254))
}

impl Drop for FixtureLoopbackServer {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

fn build_server_config(
    identity: FixtureTlsIdentity,
) -> Result<ServerConfig, DirectSyncTransportError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?
        .with_no_client_auth()
        .with_single_cert(vec![identity.certificate], identity.private_key)
        .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?;
    config.alpn_protocols = vec![HTTP_1_1_ALPN.to_vec()];
    config.session_storage = Arc::new(NoServerSessionStorage {});
    config.max_early_data_size = 0;
    config.send_half_rtt_data = false;
    config.send_tls13_tickets = 0;
    config.enable_secret_extraction = false;
    Ok(config)
}

async fn run_accept_loop<H>(
    listener: TcpListener,
    local_addr: SocketAddr,
    tls_acceptor: TlsAcceptor,
    handler: Arc<H>,
    policy: FixtureTransportPolicy,
    peer_boundary: PeerBoundary,
    mut shutdown_rx: oneshot::Receiver<()>,
) where
    H: WireRequestHandler,
{
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    // Direct-sync authority work is synchronous SQLite today. Keep it off the
    // Tokio connection tasks and serialize it through one bounded blocking
    // lane so timed-out callers cannot accumulate detached database writers.
    let blocking_gate = Arc::new(Semaphore::new(1));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { break };
                if !peer_boundary.accepts(peer.ip()) {
                    continue;
                }
                let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned() else {
                    continue;
                };
                let tls_acceptor = tls_acceptor.clone();
                let handler = Arc::clone(&handler);
                let policy = policy.clone();
                let blocking_gate = Arc::clone(&blocking_gate);
                connections.spawn(async move {
                    let _permit = permit;
                    serve_connection(
                        stream,
                        local_addr,
                        tls_acceptor,
                        handler,
                        policy,
                        blocking_gate,
                    )
                    .await;
                });
            }
        }
    }
    while connections.join_next().await.is_some() {}
}

async fn serve_connection<H>(
    stream: TcpStream,
    local_addr: SocketAddr,
    tls_acceptor: TlsAcceptor,
    handler: Arc<H>,
    policy: FixtureTransportPolicy,
    blocking_gate: Arc<Semaphore>,
) where
    H: WireRequestHandler,
{
    let Ok(Ok(mut tls)) = timeout(TLS_HANDSHAKE_TIMEOUT, tls_acceptor.accept(stream)).await else {
        return;
    };
    let used_zero_rtt = tls.get_mut().1.early_data().is_some();
    let connection = tls.get_ref().1;
    if connection.protocol_version() != Some(ProtocolVersion::TLSv1_3)
        || connection.alpn_protocol() != Some(HTTP_1_1_ALPN)
        || used_zero_rtt
    {
        return;
    }

    // Hyper intentionally accepts repeated equal Content-Length fields under
    // RFC 9110. The direct-sync contract is narrower: exactly one field must
    // appear on the wire. Inspect the bounded decrypted header before handing
    // the original bytes (plus any already-read body prefix) back to Hyper.
    let expected_authority = local_addr.to_string();
    let Ok(prefix) = preflight_http_head(
        &mut tls,
        &policy,
        &expected_authority,
        handler.pairing_enabled(),
    )
    .await
    else {
        return;
    };

    let transport = SecureTransportEvidence {
        tls_version: "1.3".to_owned(),
        used_zero_rtt,
        server_spki_sha256: policy.expected_server_spki_sha256().to_vec(),
    };
    let service = service_fn(move |request| {
        let handler = Arc::clone(&handler);
        let policy = policy.clone();
        let transport = transport.clone();
        let expected_authority = expected_authority.clone();
        let blocking_gate = Arc::clone(&blocking_gate);
        async move {
            Ok::<_, Infallible>(
                handle_http_request(
                    request,
                    handler,
                    policy,
                    transport,
                    &expected_authority,
                    blocking_gate,
                )
                .await,
            )
        }
    });

    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_headers(MAX_HEADERS)
        .max_buf_size(MAX_HEADER_BUFFER_BYTES)
        .half_close(false)
        .keep_alive(false);
    let connection = builder.serve_connection(TokioIo::new(PrefixedIo::new(prefix, tls)), service);
    let _ = timeout(CONNECTION_TIMEOUT, connection).await;
}

async fn preflight_http_head<T>(
    stream: &mut T,
    policy: &FixtureTransportPolicy,
    expected_authority: &str,
    pairing_enabled: bool,
) -> Result<Vec<u8>, ()>
where
    T: AsyncRead + Unpin,
{
    let deadline = tokio::time::Instant::now() + HEADER_TIMEOUT;
    let mut prefix = Vec::with_capacity(1024);
    let header_end = loop {
        if let Some(index) = prefix.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if prefix.len() >= MAX_HEADER_BUFFER_BYTES {
            return Err(());
        }
        let mut chunk = [0u8; 1024];
        let remaining = MAX_HEADER_BUFFER_BYTES - prefix.len();
        let read_capacity = remaining.min(chunk.len());
        let read = timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            stream.read(&mut chunk[..read_capacity]),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        if read == 0 {
            return Err(());
        }
        prefix.extend_from_slice(&chunk[..read]);
    };
    validate_raw_http_head(
        &prefix[..header_end],
        policy,
        expected_authority,
        pairing_enabled,
    )?;
    Ok(prefix)
}

fn validate_raw_http_head(
    head: &[u8],
    policy: &FixtureTransportPolicy,
    expected_authority: &str,
    pairing_enabled: bool,
) -> Result<(), ()> {
    let text = std::str::from_utf8(head).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(())?;
    let mut request_parts = request_line.split(' ');
    if request_parts.next() != Some("POST") {
        return Err(());
    }
    let target = request_parts.next().ok_or(())?;
    let endpoint = wire_endpoint_from_exact_target(target, pairing_enabled).ok_or(())?;
    if request_parts.next() != Some("HTTP/1.1") || request_parts.next().is_some() {
        return Err(());
    }

    let mut header_count = 0usize;
    let mut content_length = None;
    let mut host = None;
    let mut content_type_count = 0usize;
    for line in lines {
        if line.is_empty() {
            break;
        }
        header_count = header_count.checked_add(1).ok_or(())?;
        if header_count > MAX_HEADERS || line.starts_with([' ', '\t']) {
            return Err(());
        }
        let (name, value) = line.split_once(':').ok_or(())?;
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'!' | b'#'..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^'..=b'`' | b'|' | b'~')
            })
        {
            return Err(());
        }
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some()
                || value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(());
            }
            content_length = Some(value.parse::<usize>().map_err(|_| ())?);
        } else if name.eq_ignore_ascii_case("host") {
            if host.replace(value).is_some() {
                return Err(());
            }
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type_count += 1;
            if !matches!(
                value,
                "application/json" | "application/json; charset=utf-8"
            ) {
                return Err(());
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("te")
            || name.eq_ignore_ascii_case("trailer")
            || name.eq_ignore_ascii_case("upgrade")
            || name.eq_ignore_ascii_case("expect")
            || name.eq_ignore_ascii_case("content-encoding")
            || name.eq_ignore_ascii_case("accept-encoding")
            || name.eq_ignore_ascii_case("origin")
        {
            return Err(());
        }
    }
    if host != Some(expected_authority) || content_type_count != 1 {
        return Err(());
    }
    let content_length = content_length.ok_or(())?;
    if content_length > endpoint.limits(policy).request_bytes {
        return Err(());
    }
    Ok(())
}

struct PrefixedIo<T> {
    prefix: Vec<u8>,
    position: usize,
    inner: T,
}

impl<T> PrefixedIo<T> {
    fn new(prefix: Vec<u8>, inner: T) -> Self {
        Self {
            prefix,
            position: 0,
            inner,
        }
    }
}

impl<T> AsyncRead for PrefixedIo<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.position < self.prefix.len() {
            let available = &self.prefix[self.position..];
            let count = available.len().min(buffer.remaining());
            buffer.put_slice(&available[..count]);
            self.position += count;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl<T> AsyncWrite for PrefixedIo<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[cfg(test)]
mod private_lan_boundary_tests {
    use super::*;

    #[test]
    fn private_lan_listener_rejects_loopback_public_and_multicast_addresses() {
        for address in [
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(224, 0, 0, 251),
            Ipv4Addr::UNSPECIFIED,
        ] {
            assert!(!is_private_lan_ipv4(address), "accepted {address}");
            assert!(!PeerBoundary::PrivateLan.accepts(IpAddr::V4(address)));
        }
        for address in [
            Ipv4Addr::new(10, 0, 0, 8),
            Ipv4Addr::new(172, 16, 0, 8),
            Ipv4Addr::new(192, 168, 1, 8),
            Ipv4Addr::new(169, 254, 1, 8),
        ] {
            assert!(is_private_lan_ipv4(address), "rejected {address}");
            assert!(PeerBoundary::PrivateLan.accepts(IpAddr::V4(address)));
        }
        assert!(!PeerBoundary::PrivateLan.accepts(IpAddr::V6("fd00::8".parse().unwrap())));
    }

    #[cfg(feature = "sanitized-development-fixtures")]
    #[test]
    fn private_lan_fixture_identity_has_a_nonzero_stable_pin() {
        let identity =
            FixtureTlsIdentity::generate_for_private_lan(Ipv4Addr::new(192, 168, 1, 8)).unwrap();
        assert_ne!(identity.spki_sha256(), [0; 32]);
    }
}

async fn handle_http_request<H>(
    request: Request<Incoming>,
    handler: Arc<H>,
    policy: FixtureTransportPolicy,
    transport: SecureTransportEvidence,
    expected_authority: &str,
    blocking_gate: Arc<Semaphore>,
) -> Response<Full<Bytes>>
where
    H: WireRequestHandler,
{
    if request.version() != Version::HTTP_11 {
        return wire_error(
            StatusCode::HTTP_VERSION_NOT_SUPPORTED,
            "http_version_rejected",
        );
    }
    if request.method() != Method::POST {
        return wire_error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    }
    if request.uri().scheme().is_some() || request.uri().authority().is_some() {
        return wire_error(StatusCode::NOT_FOUND, "route_not_found");
    }
    let Some(target) = request.uri().path_and_query().map(|value| value.as_str()) else {
        return wire_error(StatusCode::NOT_FOUND, "route_not_found");
    };
    let Some(endpoint) = wire_endpoint_from_exact_target(target, handler.pairing_enabled()) else {
        return wire_error(StatusCode::NOT_FOUND, "route_not_found");
    };
    if request.headers().len() > MAX_HEADERS
        || request.headers().contains_key(TRANSFER_ENCODING)
        || request.headers().contains_key(TE)
        || request.headers().contains_key(TRAILER)
        || request.headers().contains_key(UPGRADE)
        || request.headers().contains_key(EXPECT)
        || request.headers().contains_key(CONTENT_ENCODING)
        || request.headers().contains_key(ACCEPT_ENCODING)
        || request.headers().contains_key(ORIGIN)
        || request.headers().get_all(HOST).iter().count() != 1
        || request
            .headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            != Some(expected_authority)
    {
        return wire_error(StatusCode::BAD_REQUEST, "invalid_http_framing");
    }
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if !matches!(
        content_type,
        Some("application/json") | Some("application/json; charset=utf-8")
    ) {
        return wire_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_content_type",
        );
    }
    let declared = match exact_content_length(request.headers()) {
        Ok(declared) => declared,
        Err(_) => return wire_error(StatusCode::BAD_REQUEST, "invalid_http_framing"),
    };
    let limits = endpoint.limits(&policy);
    if declared > limits.request_bytes {
        return wire_error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large");
    }

    let content_type = content_type.map(str::to_owned);
    let collected = match timeout(
        BODY_TIMEOUT,
        Limited::new(request.into_body(), limits.request_bytes).collect(),
    )
    .await
    {
        Ok(Ok(collected)) => collected,
        Ok(Err(_)) => return wire_error(StatusCode::BAD_REQUEST, "invalid_http_framing"),
        Err(_) => return wire_error(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    };
    let body = collected.to_bytes().to_vec();
    if body.len() != declared {
        return wire_error(StatusCode::BAD_REQUEST, "invalid_http_framing");
    }

    let pairing_transport = TransportEvidence {
        tls_version: transport.tls_version.clone(),
        used_zero_rtt: transport.used_zero_rtt,
        peer_spki_sha256: transport.server_spki_sha256.clone(),
    };
    let blocking_permit = match timeout(HANDLER_TIMEOUT, blocking_gate.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        _ => return wire_error(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    };
    let wire_response = match timeout(
        HANDLER_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let _blocking_permit = blocking_permit;
            match endpoint {
                WireEndpoint::Direct(endpoint) => {
                    WireResponse::Direct(handler.handle_direct_sync(DirectRequest {
                        method: "POST".to_owned(),
                        target: endpoint.path().to_owned(),
                        content_type,
                        content_encoding: None,
                        body,
                        authority_now: fixture_now_ms(),
                        transport,
                    }))
                }
                WireEndpoint::Pairing(endpoint) => {
                    WireResponse::Pairing(handler.handle_pairing(PairingTransportRequest {
                        endpoint,
                        body,
                        transport: pairing_transport,
                    }))
                }
            }
        }),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => return wire_error(StatusCode::SERVICE_UNAVAILABLE, "state_unavailable"),
        Err(_) => return wire_error(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    };
    if wire_response.body_len() > limits.response_bytes {
        return wire_error(StatusCode::PAYLOAD_TOO_LARGE, "response_too_large");
    }
    match wire_response {
        WireResponse::Direct(response) => direct_response_to_http(response),
        WireResponse::Pairing(response) => pairing_response_to_http(response),
    }
}

enum WireResponse {
    Direct(DirectResponse),
    Pairing(PairingTransportResponse),
}

impl WireResponse {
    fn body_len(&self) -> usize {
        match self {
            Self::Direct(response) => response.body.len(),
            Self::Pairing(response) => response.body.len(),
        }
    }
}

fn direct_response_to_http(response: DirectResponse) -> Response<Full<Bytes>> {
    if response.content_type != DIRECT_SYNC_CONTENT_TYPE {
        return wire_error(StatusCode::SERVICE_UNAVAILABLE, "state_unavailable");
    }
    let Ok(status) = StatusCode::from_u16(response.status) else {
        return wire_error(StatusCode::SERVICE_UNAVAILABLE, "state_unavailable");
    };
    build_response(status, response.body)
}

fn pairing_response_to_http(response: PairingTransportResponse) -> Response<Full<Bytes>> {
    let Ok(status) = StatusCode::from_u16(response.status) else {
        return wire_error(StatusCode::SERVICE_UNAVAILABLE, "state_unavailable");
    };
    build_response(status, response.body)
}

fn wire_error(status: StatusCode, code: &'static str) -> Response<Full<Bytes>> {
    let body = format!("{{\"error\":{{\"code\":\"{code}\"}}}}").into_bytes();
    build_response(status, body)
}

fn build_response(status: StatusCode, body: Vec<u8>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, DIRECT_SYNC_CONTENT_TYPE)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header(CONNECTION, "close")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

fn fixture_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) fn fixture_server_config_for_test(
    identity: FixtureTlsIdentity,
) -> Result<ServerConfig, DirectSyncTransportError> {
    build_server_config(identity)
}
