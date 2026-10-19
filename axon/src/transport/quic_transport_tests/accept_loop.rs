//! Regression coverage for the inbound accept path:
//!
//! 1. A half-open, unauthenticated handshake must not delay later inbound
//!    connections: the accept loop hands every handshake to its own tracked
//!    task instead of awaiting it inline.
//! 2. A real handshake that started before `revoke_peer` committed must be
//!    rejected at admission by the per-peer enrollment epoch, even when the
//!    peer was re-enrolled while the revocation pair was parked. The
//!    existing epoch tests drive the gate synthetically; this interleaves a
//!    real mTLS handshake with `revoke_peer`.
//! 3. The pre-authentication handshake bound must saturate: once every
//!    pre-auth permit is held by a handshake that has not finished, the next
//!    inbound handshake is refused outright (`Incoming::refuse`, i.e. a
//!    transport-level `CONNECTION_REFUSED`) instead of being queued.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::json;

use super::fixtures::{make_transport_pair, make_transport_pair_with_options};
use crate::identity::Identity;
use crate::message::{AgentId, Envelope, MessageKind};
use crate::peer_directory::{PeerIdentity, PeerLocator};

/// Client certificate verifier that parks inside server-certificate
/// verification — before the client sends its own certificate — keeping the
/// server-side handshake mid-flight until the test releases it. Returning
/// `Ok` after release lets the handshake complete so server admission runs.
#[derive(Debug)]
struct StallingVerifier {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl StallingVerifier {
    fn pair() -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        (
            Arc::new(Self {
                entered: Mutex::new(Some(entered_tx)),
                release: Mutex::new(release_rx),
            }),
            entered_rx,
            release_tx,
        )
    }
}

impl ServerCertVerifier for StallingVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(tx) = self.entered.lock().expect("entered lock").take() {
            let _ = tx.send(());
        }
        // Blocks until the test releases (or drops the sender).
        let _ = self.release.lock().expect("release lock").recv();
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 not supported".to_string()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Client certificate verifier that accepts any server certificate, so a
/// plain client can complete a normal handshake against the test daemons.
#[derive(Debug)]
struct AcceptAnyVerifier;

impl ServerCertVerifier for AcceptAnyVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 not supported".to_string()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn client_endpoint(identity: &Identity, verifier: Arc<dyn ServerCertVerifier>) -> quinn::Endpoint {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let cert = identity
        .make_quic_certificate()
        .expect("client certificate");
    let cert_chain = vec![CertificateDer::from(cert.cert_der)];
    let key = PrivatePkcs8KeyDer::from(cert.key_der);
    let mut config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(cert_chain, key.into())
        .expect("client auth config");
    config.alpn_protocols = vec![b"axon/1".to_vec()];
    config.enable_early_data = false;
    let quic =
        quinn::crypto::rustls::QuicClientConfig::try_from(config).expect("quic client config");
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic));
    client_config.transport_config(Arc::new(quinn::TransportConfig::default()));
    let mut endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).expect("client endpoint");
    endpoint.set_default_client_config(client_config);
    endpoint
}

/// Spawn a client using `identity` that connects to `addr` and stalls inside
/// server-certificate verification. The returned receiver fires once the
/// handshake has reached the verifier (so the server is guaranteed mid-flight
/// and has already captured its admission epoch); the returned sender
/// releases the stall.
type StallingClient = (
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
);

fn spawn_stalling_client(
    identity: Identity,
    addr: SocketAddr,
    server_name: String,
) -> StallingClient {
    let (stall, entered_rx, release_tx) = StallingVerifier::pair();
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("client runtime");
        runtime.block_on(async move {
            let endpoint = client_endpoint(&identity, stall);
            match endpoint.connect(addr, &server_name) {
                Ok(connecting) => {
                    if let Ok(connection) = connecting.await {
                        connection.close(0u32.into(), b"test complete");
                    }
                }
                Err(err) => panic!("stalling client failed to start: {err}"),
            }
            endpoint.close(0u32.into(), b"test complete");
            endpoint.wait_idle().await;
        });
    });
    (entered_rx, release_tx, handle)
}

async fn wait_for_verifier(entered_rx: mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .expect("join")
        .expect("stalled handshake must reach the client certificate verifier");
}

async fn release_client(release_tx: mpsc::Sender<()>, handle: std::thread::JoinHandle<()>) {
    let _ = release_tx.send(());
    let _ = tokio::task::spawn_blocking(move || handle.join()).await;
}

/// Attempt one complete handshake with a client that accepts any server
/// certificate. Returns how long the attempt took and its outcome, so a test
/// can distinguish a prompt refusal from a slow timeout.
async fn attempt_connection(
    identity: &Identity,
    addr: SocketAddr,
    server_name: &str,
) -> (Duration, Result<quinn::Connection, quinn::ConnectionError>) {
    let endpoint = client_endpoint(identity, Arc::new(AcceptAnyVerifier));
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        endpoint.connect(addr, server_name).expect("connect start"),
    )
    .await
    .expect("connection attempt must resolve instead of hanging");
    let elapsed = started.elapsed();
    endpoint.close(0u32.into(), b"test attempt complete");
    (elapsed, result)
}

/// True when the server refused the handshake before authentication, which
/// is the only thing that produces a transport-level `CONNECTION_REFUSED`.
fn refused_before_auth(result: &Result<quinn::Connection, quinn::ConnectionError>) -> bool {
    matches!(
        result,
        Err(quinn::ConnectionError::ConnectionClosed(close))
            if close.error_code == quinn::TransportErrorCode::CONNECTION_REFUSED
    )
}

#[tokio::test]
async fn stalled_handshake_does_not_block_later_inbound_connections() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr_b = pair.transport_b.local_addr().unwrap();

    // Park one real inbound handshake at B mid-flight.
    let (entered_rx, release_tx, handle) =
        spawn_stalling_client(pair.id_a.clone(), addr_b, pair.id_b.agent_id().to_string());
    wait_for_verifier(entered_rx).await;

    // A second, fully legitimate connection must still be accepted and
    // admitted promptly while the first handshake is stalled.
    let message = Envelope::new(
        agent_a.clone(),
        agent_b.clone(),
        MessageKind::Message,
        json!({"probe": true}),
    );
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, message, Duration::from_secs(5))
        .await
        .expect("later handshake must not be blocked by the stalled one");

    let deadline = Instant::now() + Duration::from_secs(2);
    while !pair.transport_b.has_connection(&agent_a).await && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        pair.transport_b.has_connection(&agent_a).await,
        "B must admit the later connection while the first handshake is stalled"
    );

    release_client(release_tx, handle).await;
}

#[tokio::test]
async fn saturated_preauth_handshake_bound_refuses_further_inbound_handshakes() {
    // B tolerates exactly one pre-authentication handshake, so a single
    // parked handshake saturates the bound.
    let pair = make_transport_pair_with_options(128, 1, None).await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();
    let addr_b = pair.transport_b.local_addr().unwrap();
    let server_name = pair.id_b.agent_id().to_string();

    // Park one real inbound handshake at B mid-flight: it holds B's only
    // pre-auth permit until the test releases it.
    let (entered_rx, release_tx, handle) =
        spawn_stalling_client(pair.id_a.clone(), addr_b, server_name.clone());
    wait_for_verifier(entered_rx).await;

    // The next inbound handshake must be refused at the accept path, i.e. it
    // fails fast with a transport-level CONNECTION_REFUSED rather than
    // waiting for a permit or being admitted.
    let (elapsed, result) = attempt_connection(&pair.id_a, addr_b, &server_name).await;
    assert!(
        refused_before_auth(&result),
        "a saturated pre-auth handshake bound must refuse the next handshake, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "a saturated bound must refuse promptly, took {elapsed:?}"
    );
    assert!(
        !pair.transport_b.has_connection(&agent_a).await,
        "a refused pre-auth handshake must not be admitted"
    );

    // Releasing the parked handshake frees the permit: the bound refuses only
    // while it is actually saturated, so later attempts are no longer
    // refused before authentication.
    release_client(release_tx, handle).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, result) = attempt_connection(&pair.id_a, addr_b, &server_name).await;
        if !refused_before_auth(&result) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "pre-auth handshake permit must be released after the parked handshake finishes"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn handshake_racing_revoke_is_rejected_after_reenroll() {
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let addr_a = pair.transport_a.local_addr().unwrap();

    // 1. Commit removal but park the revocation pair BEFORE transport
    //    teardown, which is what bumps B's enrollment epoch.
    pair.transport_a
        .revocation_pause
        .armed
        .store(true, Ordering::Release);
    let revoker = {
        let transport = pair.transport_a.clone();
        let agent_b = agent_b.clone();
        tokio::spawn(async move { transport.revoke_peer(&agent_b).await })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while pair.directory_a.get_enrolled(&agent_b).await.is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(
        pair.directory_a.get_enrolled(&agent_b).await.is_none(),
        "directory commit must land while the revocation pair is parked"
    );

    // 2. Re-enroll B while the epoch bump is still parked: the pin snapshot
    //    lists B again, so only the epoch can reject a stale handshake.
    let identity_b = PeerIdentity::from_public_key(pair.id_b.public_key_base64()).unwrap();
    pair.directory_a
        .enroll(
            identity_b,
            vec![PeerLocator::Socket(pair.transport_b.local_addr().unwrap())],
        )
        .await
        .expect("re-enroll B");

    // 3. Start a real inbound handshake from B and park it at its
    //    certificate verification. A's accept loop captures B's epoch (still
    //    0) when it picks up the Incoming, before the handshake begins.
    let (entered_rx, release_tx, handle) =
        spawn_stalling_client(pair.id_b.clone(), addr_a, pair.id_a.agent_id().to_string());
    wait_for_verifier(entered_rx).await;

    // 4. Release the revocation: the epoch bumps to 1, and the empty
    //    registry is torn down. Awaiting the task guarantees the bump landed
    //    before the handshake may finish.
    pair.transport_a
        .revocation_pause
        .armed
        .store(false, Ordering::Release);
    pair.transport_a.revocation_pause.release.notify_one();
    revoker
        .await
        .expect("revocation task")
        .expect("directory removal");

    // 5. Let the handshake complete: admission must reject it on the epoch
    //    mismatch even though B is currently enrolled again.
    release_client(release_tx, handle).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        !pair.transport_a.has_connection(&agent_b).await,
        "a handshake that started before revocation must not be admitted after re-enrollment"
    );
    assert!(
        pair.directory_a.get_enrolled(&agent_b).await.is_some(),
        "test requires B to remain enrolled after re-enrollment"
    );
}
