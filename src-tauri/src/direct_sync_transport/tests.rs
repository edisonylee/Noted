use super::client::{fixture_client_config_for_test, fixture_p256_spki_der_for_test};
use super::server::fixture_server_config_for_test;
use super::*;
use crate::direct_sync::{
    DirectRequest, DirectResponse, DirectSyncLimits, DIRECT_SYNC_CONTENT_TYPE,
};
use rustls::pki_types::ServerName;
use sha2::Digest;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

#[derive(Default)]
struct RecordingHandler {
    requests: Mutex<Vec<DirectRequest>>,
    pairing_requests: Mutex<Vec<PairingTransportRequest>>,
    oversized_response: Mutex<bool>,
}

impl RecordingHandler {
    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn requests(&self) -> Vec<DirectRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn make_next_response_oversized(&self) {
        *self.oversized_response.lock().unwrap() = true;
    }

    fn pairing_requests(&self) -> Vec<PairingTransportRequest> {
        self.pairing_requests.lock().unwrap().clone()
    }
}

impl FixtureAuthorityRequestHandler for RecordingHandler {
    fn handle_pairing(&self, request: PairingTransportRequest) -> PairingTransportResponse {
        self.pairing_requests.lock().unwrap().push(request);
        PairingTransportResponse {
            status: 200,
            body: br#"{"paired":true}"#.to_vec(),
        }
    }
}

impl DirectSyncRequestHandler for RecordingHandler {
    fn handle_direct_sync(&self, request: DirectRequest) -> DirectResponse {
        self.requests.lock().unwrap().push(request);
        let body = if *self.oversized_response.lock().unwrap() {
            vec![b'x'; 65 * 1024]
        } else {
            br#"{"ok":true}"#.to_vec()
        };
        DirectResponse {
            status: 200,
            content_type: DIRECT_SYNC_CONTENT_TYPE,
            body,
        }
    }
}

async fn fixture() -> (
    Arc<RecordingHandler>,
    FixtureTransportPolicy,
    FixtureLoopbackServer,
    FixtureDirectSyncClient,
) {
    let handler = Arc::new(RecordingHandler::default());
    let identity = FixtureTlsIdentity::generate().unwrap();
    let policy = FixtureTransportPolicy::new_fixture_only(
        identity.spki_sha256(),
        DirectSyncLimits::default(),
    )
    .unwrap();
    let server =
        FixtureLoopbackServer::spawn_fixture_only(Arc::clone(&handler), identity, policy.clone())
            .await
            .unwrap();
    let client =
        FixtureDirectSyncClient::new_fixture_only(server.local_addr(), policy.clone()).unwrap();
    (handler, policy, server, client)
}

#[tokio::test]
async fn all_six_typed_routes_cross_real_tls13_http11() {
    let (handler, policy, server, client) = fixture().await;

    for endpoint in DirectEndpoint::ALL {
        let response = client.post(endpoint, br#"{}"#.to_vec()).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"ok":true}"#);
    }

    let requests = handler.requests();
    assert_eq!(requests.len(), DirectEndpoint::ALL.len());
    for (request, endpoint) in requests.iter().zip(DirectEndpoint::ALL) {
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, endpoint.path());
        assert_eq!(request.content_type.as_deref(), Some("application/json"));
        assert_eq!(request.content_encoding, None);
        assert_eq!(request.transport.tls_version, "1.3");
        assert!(!request.transport.used_zero_rtt);
        assert_eq!(
            request.transport.server_spki_sha256,
            policy.expected_server_spki_sha256()
        );
    }
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_pin_and_plaintext_never_reach_the_router() {
    let (handler, policy, server, _client) = fixture().await;
    let wrong_policy =
        FixtureTransportPolicy::new_fixture_only([0x55; 32], policy.limits().clone()).unwrap();
    let wrong_client =
        FixtureDirectSyncClient::new_fixture_only(server.local_addr(), wrong_policy).unwrap();
    assert_eq!(
        wrong_client
            .post(DirectEndpoint::Negotiate, br#"{}"#.to_vec())
            .await,
        Err(DirectSyncTransportError::SecureTransportFailed)
    );

    let mut plaintext = TcpStream::connect(server.local_addr()).await.unwrap();
    plaintext
        .write_all(valid_request(server.local_addr(), "/sync/v1/negotiate").as_bytes())
        .await
        .unwrap();
    plaintext.shutdown().await.unwrap();
    let mut ignored = Vec::new();
    let _ = timeout(Duration::from_secs(1), plaintext.read_to_end(&mut ignored)).await;
    assert_eq!(handler.request_count(), 0);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn target_aliases_are_rejected_before_the_router() {
    let (handler, policy, server, _client) = fixture().await;
    let targets = [
        "/sync/v1/negotiate/",
        "/sync/v1/negotiate?token=nope",
        "/sync/v1/NEGOTIATE",
        "/sync/v1/%6egotiate",
        "https://127.0.0.1/sync/v1/negotiate",
        "/api/get_notes",
        "/pair/v1/start",
    ];
    for target in targets {
        let response = raw_tls_request(
            server.local_addr(),
            &policy,
            valid_request(server.local_addr(), target).as_bytes(),
        )
        .await;
        if !response.is_empty() {
            assert!(response.starts_with(b"HTTP/1.1 404"), "{target}");
        }
    }
    assert_eq!(handler.request_count(), 0);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn sync_only_listener_rejects_all_pairing_routes_before_the_router() {
    let (handler, policy, server, _client) = fixture().await;
    for endpoint in [
        PairingEndpoint::ClientHello,
        PairingEndpoint::Bootstrap,
        PairingEndpoint::ClientFinish,
    ] {
        let response = raw_tls_request(
            server.local_addr(),
            &policy,
            valid_request(server.local_addr(), endpoint.path()).as_bytes(),
        )
        .await;
        assert!(response.is_empty());
    }
    assert_eq!(handler.request_count(), 0);
    assert!(handler.pairing_requests().is_empty());
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_authority_listener_routes_only_fixed_pairing_paths_with_native_tls_evidence() {
    let handler = Arc::new(RecordingHandler::default());
    let identity = FixtureTlsIdentity::generate().unwrap();
    let policy = FixtureTransportPolicy::new_fixture_only(
        identity.spki_sha256(),
        DirectSyncLimits::default(),
    )
    .unwrap();
    let server = FixtureLoopbackServer::spawn_authority_fixture_only(
        Arc::clone(&handler),
        identity,
        policy.clone(),
    )
    .await
    .unwrap();

    for endpoint in [
        PairingEndpoint::ClientHello,
        PairingEndpoint::Bootstrap,
        PairingEndpoint::ClientFinish,
    ] {
        let response = raw_tls_request(
            server.local_addr(),
            &policy,
            valid_request(server.local_addr(), endpoint.path()).as_bytes(),
        )
        .await;
        assert!(response.starts_with(b"HTTP/1.1 200"));
    }

    let requests = handler.pairing_requests();
    assert_eq!(requests.len(), 3);
    for (request, endpoint) in requests.iter().zip([
        PairingEndpoint::ClientHello,
        PairingEndpoint::Bootstrap,
        PairingEndpoint::ClientFinish,
    ]) {
        assert_eq!(request.endpoint, endpoint);
        assert_eq!(request.body, br#"{}"#);
        assert_eq!(request.transport.tls_version, "1.3");
        assert!(!request.transport.used_zero_rtt);
        assert_eq!(
            request.transport.peer_spki_sha256,
            policy.expected_server_spki_sha256()
        );
    }

    let alias = raw_tls_request(
        server.local_addr(),
        &policy,
        valid_request(server.local_addr(), "/pairing/v1/client-hello/").as_bytes(),
    )
    .await;
    assert!(alias.is_empty());
    assert_eq!(handler.pairing_requests().len(), 3);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_host_and_browser_origin_are_rejected_before_the_router() {
    let (handler, policy, server, _client) = fixture().await;
    let address = server.local_addr();
    let wrong_host = "POST /sync/v1/negotiate HTTP/1.1\r\nHost: attacker.invalid\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string();
    let browser_origin = format!(
        "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {address}\r\nOrigin: https://example.invalid\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    let _ = raw_tls_request(address, &policy, wrong_host.as_bytes()).await;
    let _ = raw_tls_request(address, &policy, browser_origin.as_bytes()).await;
    assert_eq!(handler.request_count(), 0);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_http_body_framing_is_rejected_before_the_router() {
    let (handler, policy, server, _client) = fixture().await;
    let address = server.local_addr();
    let host = address.to_string();
    let requests = [
        format!(
            "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n"
        ),
        format!(
            "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
        format!(
            "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{{}}"
        ),
        format!(
            "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Encoding: gzip\r\nConnection: close\r\n\r\n{{}}"
        ),
        format!(
            "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n{{}}"
        ),
        format!(
            "POST /sync/v1/negotiate HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 9000\r\nConnection: close\r\n\r\n"
        ),
    ];

    for (index, request) in requests.into_iter().enumerate() {
        let _ = raw_tls_request(address, &policy, request.as_bytes()).await;
        assert_eq!(handler.request_count(), 0, "framing case {index}");
    }
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_alpn_never_reaches_the_router() {
    let (handler, policy, server, _client) = fixture().await;
    let mut config = fixture_client_config_for_test(&policy).unwrap();
    config.alpn_protocols = vec![b"h2".to_vec()];
    let tcp = TcpStream::connect(server.local_addr()).await.unwrap();
    let result = TlsConnector::from(Arc::new(config))
        .connect(server_name(server.local_addr().ip()), tcp)
        .await;
    assert!(result.is_err());
    assert_eq!(handler.request_count(), 0);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn route_response_cap_is_enforced_by_the_server() {
    let (handler, _policy, server, client) = fixture().await;
    handler.make_next_response_oversized();
    let response = client
        .post(DirectEndpoint::Negotiate, br#"{}"#.to_vec())
        .await
        .unwrap();
    assert_eq!(response.status, 413);
    assert!(String::from_utf8(response.body)
        .unwrap()
        .contains("response_too_large"));
    assert_eq!(handler.request_count(), 1);
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_the_bound_loopback_listener() {
    let (_handler, _policy, server, _client) = fixture().await;
    let address = server.local_addr();
    server.shutdown().await.unwrap();
    assert!(TcpStream::connect(address).await.is_err());
}

#[test]
fn fixture_policy_cannot_encode_production_or_unbounded_limits() {
    assert_eq!(
        FixtureTransportPolicy::new_fixture_only([0; 32], DirectSyncLimits::default()),
        Err(DirectSyncTransportError::InvalidFixtureConfiguration)
    );
    let mut limits = DirectSyncLimits::default();
    limits.negotiate.request_bytes = 0;
    assert_eq!(
        FixtureTransportPolicy::new_fixture_only([1; 32], limits),
        Err(DirectSyncTransportError::InvalidFixtureConfiguration)
    );
}

#[test]
fn tls_configs_disable_early_data_resumption_and_tickets() {
    let identity = FixtureTlsIdentity::generate().unwrap();
    let policy = FixtureTransportPolicy::new_fixture_only(
        identity.spki_sha256(),
        DirectSyncLimits::default(),
    )
    .unwrap();
    let client = fixture_client_config_for_test(&policy).unwrap();
    assert!(!client.enable_early_data);
    assert!(!client.enable_secret_extraction);
    assert_eq!(client.alpn_protocols, vec![HTTP_1_1_ALPN.to_vec()]);

    let server = fixture_server_config_for_test(identity).unwrap();
    assert_eq!(server.max_early_data_size, 0);
    assert!(!server.send_half_rtt_data);
    assert_eq!(server.send_tls13_tickets, 0);
    assert!(!server.enable_secret_extraction);
    assert_eq!(server.alpn_protocols, vec![HTTP_1_1_ALPN.to_vec()]);
}

#[test]
fn pin_is_the_exact_der_encoded_p256_spki() {
    let identity = FixtureTlsIdentity::generate().unwrap();
    let spki = fixture_p256_spki_der_for_test(identity.certificate_der_for_test()).unwrap();
    let digest: [u8; 32] = sha2::Sha256::digest(spki).into();
    assert_eq!(digest, identity.spki_sha256());
    assert_eq!(fixture_p256_spki_der_for_test(b"not a certificate"), None);
}

async fn raw_tls_request(
    address: std::net::SocketAddr,
    policy: &FixtureTransportPolicy,
    request: &[u8],
) -> Vec<u8> {
    let tcp = TcpStream::connect(address).await.unwrap();
    let mut tls = TlsConnector::from(Arc::new(fixture_client_config_for_test(policy).unwrap()))
        .connect(server_name(address.ip()), tcp)
        .await
        .unwrap();
    tls.write_all(request).await.unwrap();
    tls.flush().await.unwrap();
    let mut response = Vec::new();
    let _ = timeout(Duration::from_secs(2), tls.read_to_end(&mut response)).await;
    response
}

fn server_name(address: IpAddr) -> ServerName<'static> {
    ServerName::IpAddress(address.into())
}

fn valid_request(address: std::net::SocketAddr, target: &str) -> String {
    format!(
        "POST {target} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    )
}
