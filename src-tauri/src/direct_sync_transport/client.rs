use super::{DirectSyncTransportError, FixtureTransportPolicy, HTTP_1_1_ALPN, MAX_HEADERS};
use crate::direct_sync::{DirectEndpoint, DirectResponse, DIRECT_SYNC_CONTENT_TYPE};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, EXPECT, TRANSFER_ENCODING,
};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::Resumption;
use rustls::crypto::{verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error as RustlsError, ProtocolVersion,
    SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use x509_cert::der::asn1::ObjectIdentifier;
use x509_cert::der::{Decode, Encode};
use x509_cert::Certificate;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const EC_PUBLIC_KEY_OID: &str = "1.2.840.10045.2.1";
const P256_CURVE_OID: &str = "1.2.840.10045.3.1.7";

/// A one-request-per-connection fixture client. It accepts a typed endpoint
/// and numeric loopback address instead of a URL, so redirects, proxies,
/// cookies, scheme fallback, and caller-selected paths cannot enter the flow.
#[derive(Clone)]
pub struct FixtureDirectSyncClient {
    address: SocketAddr,
    policy: FixtureTransportPolicy,
    tls_config: Arc<ClientConfig>,
}

impl FixtureDirectSyncClient {
    pub fn new_fixture_only(
        address: SocketAddr,
        policy: FixtureTransportPolicy,
    ) -> Result<Self, DirectSyncTransportError> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(DirectSyncTransportError::LoopbackRequired);
        }
        let tls_config = Arc::new(build_client_config(&policy)?);
        Ok(Self {
            address,
            policy,
            tls_config,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Sends exactly one POST over a fresh TCP, TLS, and HTTP/1.1 connection.
    /// It deliberately performs no implicit retry because replay policy belongs
    /// to the signed sync protocol rather than the network stack.
    pub async fn post(
        &self,
        endpoint: DirectEndpoint,
        body: Vec<u8>,
    ) -> Result<DirectResponse, DirectSyncTransportError> {
        let limits = self.policy.limits_for(endpoint);
        if body.len() > limits.request_bytes {
            return Err(DirectSyncTransportError::RequestTooLarge);
        }

        let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(self.address))
            .await
            .map_err(|_| DirectSyncTransportError::TimedOut)?
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        let server_name = server_name_for(self.address.ip());
        let tls = timeout(
            HANDSHAKE_TIMEOUT,
            TlsConnector::from(Arc::clone(&self.tls_config)).connect(server_name, tcp),
        )
        .await
        .map_err(|_| DirectSyncTransportError::TimedOut)?
        .map_err(|_| DirectSyncTransportError::SecureTransportFailed)?;

        let connection = tls.get_ref().1;
        if connection.protocol_version() != Some(ProtocolVersion::TLSv1_3)
            || connection.alpn_protocol() != Some(HTTP_1_1_ALPN)
            || connection.is_early_data_accepted()
        {
            return Err(DirectSyncTransportError::SecureTransportFailed);
        }

        let io = TokioIo::new(tls);
        let (mut sender, connection) =
            timeout(HANDSHAKE_TIMEOUT, hyper::client::conn::http1::handshake(io))
                .await
                .map_err(|_| DirectSyncTransportError::TimedOut)?
                .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        let connection_task = tokio::spawn(connection);

        let request = Request::builder()
            .method("POST")
            .uri(endpoint.path())
            .header("host", authority_header(self.address))
            .header(CONTENT_TYPE, DIRECT_SYNC_CONTENT_TYPE)
            .header(CONTENT_LENGTH, body.len().to_string())
            .header(CONNECTION, "close")
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| DirectSyncTransportError::InvalidHttpFraming)?;

        let response = timeout(RESPONSE_TIMEOUT, sender.send_request(request))
            .await
            .map_err(|_| DirectSyncTransportError::TimedOut)?
            .map_err(|_| DirectSyncTransportError::ConnectionFailed)?;
        drop(sender);
        let result = collect_response(response, limits.response_bytes).await;
        connection_task.abort();
        result
    }
}

pub(super) fn build_client_config(
    policy: &FixtureTransportPolicy,
) -> Result<ClientConfig, DirectSyncTransportError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algorithms = provider.signature_verification_algorithms;
    let verifier = Arc::new(PinnedP256SpkiVerifier {
        expected_spki_sha256: policy.expected_server_spki_sha256(),
        algorithms,
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| DirectSyncTransportError::InvalidFixtureConfiguration)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![HTTP_1_1_ALPN.to_vec()];
    config.enable_early_data = false;
    config.resumption = Resumption::disabled();
    config.enable_sni = false;
    config.enable_secret_extraction = false;
    Ok(config)
}

pub(super) async fn collect_response(
    response: Response<Incoming>,
    response_limit: usize,
) -> Result<DirectResponse, DirectSyncTransportError> {
    if response.headers().len() > MAX_HEADERS
        || response.headers().contains_key(TRANSFER_ENCODING)
        || response.headers().contains_key(CONTENT_ENCODING)
        || response.headers().contains_key(EXPECT)
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(DIRECT_SYNC_CONTENT_TYPE)
    {
        return Err(DirectSyncTransportError::InvalidHttpFraming);
    }
    let declared = exact_content_length(response.headers())?;
    if declared > response_limit {
        return Err(DirectSyncTransportError::ResponseTooLarge);
    }
    let status = response.status().as_u16();
    let collected = timeout(
        RESPONSE_TIMEOUT,
        Limited::new(response.into_body(), response_limit).collect(),
    )
    .await
    .map_err(|_| DirectSyncTransportError::TimedOut)?
    .map_err(|_| DirectSyncTransportError::InvalidHttpFraming)?;
    let body = collected.to_bytes().to_vec();
    if body.len() != declared {
        return Err(DirectSyncTransportError::InvalidHttpFraming);
    }
    Ok(DirectResponse {
        status,
        content_type: DIRECT_SYNC_CONTENT_TYPE,
        body,
    })
}

pub(crate) fn exact_content_length(
    headers: &hyper::HeaderMap,
) -> Result<usize, DirectSyncTransportError> {
    let mut values = headers.get_all(CONTENT_LENGTH).iter();
    let first = values
        .next()
        .ok_or(DirectSyncTransportError::InvalidHttpFraming)?;
    if values.next().is_some() {
        return Err(DirectSyncTransportError::InvalidHttpFraming);
    }
    let text = first
        .to_str()
        .map_err(|_| DirectSyncTransportError::InvalidHttpFraming)?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DirectSyncTransportError::InvalidHttpFraming);
    }
    text.parse()
        .map_err(|_| DirectSyncTransportError::InvalidHttpFraming)
}

fn server_name_for(address: IpAddr) -> ServerName<'static> {
    ServerName::IpAddress(address.into())
}

pub(super) fn authority_header(address: SocketAddr) -> String {
    match address {
        SocketAddr::V4(_) => address.to_string(),
        SocketAddr::V6(_) => format!("[{}]:{}", address.ip(), address.port()),
    }
}

struct PinnedP256SpkiVerifier {
    expected_spki_sha256: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for PinnedP256SpkiVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedP256SpkiVerifier")
            .finish_non_exhaustive()
    }
}

impl ServerCertVerifier for PinnedP256SpkiVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        if !intermediates.is_empty() || !ocsp_response.is_empty() {
            return Err(RustlsError::InvalidCertificate(
                CertificateError::BadEncoding,
            ));
        }
        let spki_der = p256_spki_der(end_entity.as_ref())
            .ok_or_else(|| RustlsError::InvalidCertificate(CertificateError::BadEncoding))?;
        let actual: [u8; 32] = Sha256::digest(&spki_der).into();
        if actual != self.expected_spki_sha256 {
            return Err(RustlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Err(RustlsError::General("TLS 1.2 is disabled".to_owned()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        if dss.scheme != SignatureScheme::ECDSA_NISTP256_SHA256 {
            return Err(RustlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

fn p256_spki_der(certificate_der: &[u8]) -> Option<Vec<u8>> {
    let certificate = Certificate::from_der(certificate_der).ok()?;
    let spki = &certificate.tbs_certificate.subject_public_key_info;
    if spki.algorithm.oid.to_string() != EC_PUBLIC_KEY_OID {
        return None;
    }
    let curve = spki
        .algorithm
        .parameters
        .as_ref()?
        .decode_as::<ObjectIdentifier>()
        .ok()?;
    if curve.to_string() != P256_CURVE_OID {
        return None;
    }
    let public_key = spki.subject_public_key.as_bytes()?;
    if public_key.len() != 65 || public_key.first() != Some(&0x04) {
        return None;
    }
    spki.to_der().ok()
}

#[cfg(test)]
pub(crate) fn fixture_client_config_for_test(
    policy: &FixtureTransportPolicy,
) -> Result<ClientConfig, DirectSyncTransportError> {
    build_client_config(policy)
}

#[cfg(test)]
pub(crate) fn fixture_p256_spki_der_for_test(certificate_der: &[u8]) -> Option<Vec<u8>> {
    p256_spki_der(certificate_der)
}
