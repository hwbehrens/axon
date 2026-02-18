use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::DistinguishedName;
use rustls::client::danger::{ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use tracing::warn;
use x509_parser::prelude::*;

use crate::identity::QuicCertificate;
use crate::message::Envelope;
use crate::peer_table::PubkeyMap;
use crate::transport::PairRequest;

static CRYPTO_PROVIDER: OnceLock<()> = OnceLock::new();
tokio::task_local! {
    static HANDSHAKE_REMOTE_ADDR: SocketAddr;
}

fn ensure_crypto_provider() {
    CRYPTO_PROVIDER.get_or_init(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
    });
}

fn canonicalize_agent_id(input: &str) -> Option<String> {
    let (prefix, hex) = input.split_once('.')?;
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "{}.{}",
        prefix.to_ascii_lowercase(),
        hex.to_ascii_lowercase()
    ))
}

fn lookup_expected_pubkey<'a>(
    expected: &'a HashMap<String, String>,
    canonical_agent_id: &str,
) -> Option<&'a String> {
    expected.get(canonical_agent_id).or_else(|| {
        expected.iter().find_map(|(agent_id, pubkey)| {
            canonicalize_agent_id(agent_id)
                .as_deref()
                .is_some_and(|id| id == canonical_agent_id)
                .then_some(pubkey)
        })
    })
}

pub(crate) async fn with_handshake_remote_addr<F, T>(addr: SocketAddr, fut: F) -> T
where
    F: Future<Output = T>,
{
    HANDSHAKE_REMOTE_ADDR.scope(addr, fut).await
}

fn current_handshake_remote_addr() -> Option<SocketAddr> {
    HANDSHAKE_REMOTE_ADDR.try_with(|addr| *addr).ok()
}

pub(crate) fn build_endpoint(
    bind_addr: SocketAddr,
    cert: &QuicCertificate,
    expected_pubkeys: PubkeyMap,
    keepalive: Duration,
    idle_timeout: Duration,
) -> Result<(
    quinn::Endpoint,
    broadcast::Sender<Arc<Envelope>>,
    broadcast::Sender<PairRequest>,
)> {
    ensure_crypto_provider();

    let cert_chain = vec![CertificateDer::from(cert.cert_der.clone())];
    let private_key = PrivatePkcs8KeyDer::from(cert.key_der.clone());

    let (pair_request_tx, _) = broadcast::channel(512);
    let pair_request_seen = Arc::new(Mutex::new(HashMap::new()));

    let subject_dn = extract_subject_dn_from_cert_der(&cert.cert_der)
        .context("failed to extract certificate subject for mTLS")?;
    let mtls_verifier = PeerClientCertVerifier {
        expected_pubkeys: expected_pubkeys.clone(),
        roots: vec![DistinguishedName::from(subject_dn)],
        pair_request_tx: pair_request_tx.clone(),
        pair_request_seen: pair_request_seen.clone(),
    };

    let mut rustls_server = rustls::ServerConfig::builder()
        .with_client_cert_verifier(Arc::new(mtls_verifier))
        .with_single_cert(cert_chain.clone(), private_key.clone_key().into())
        .context("failed to build rustls server config")?;
    rustls_server.alpn_protocols = vec![b"axon/1".to_vec()];
    rustls_server.max_early_data_size = 0;

    let quic_server_config = QuicServerConfig::try_from(rustls_server)
        .context("failed to build QUIC server config from rustls")?;
    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server_config));

    let transport_config = Arc::new({
        let mut config = quinn::TransportConfig::default();
        config.keep_alive_interval(Some(keepalive));
        config.max_concurrent_bidi_streams(8u32.into());
        config.max_concurrent_uni_streams(16u32.into());
        if let Ok(idle) = quinn::IdleTimeout::try_from(idle_timeout) {
            config.max_idle_timeout(Some(idle));
        }
        config
    });
    server_config.transport = transport_config.clone();

    let mut endpoint = quinn::Endpoint::server(server_config, bind_addr)
        .with_context(|| format!("failed to bind QUIC endpoint on {bind_addr}"))?;

    let mut rustls_client = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PeerCertVerifier {
            expected_pubkeys,
            pair_request_tx: pair_request_tx.clone(),
            pair_request_seen,
        }))
        .with_client_auth_cert(cert_chain, private_key.into())
        .context("failed to configure client mTLS certificate")?;
    rustls_client.alpn_protocols = vec![b"axon/1".to_vec()];
    rustls_client.enable_early_data = false;

    let quic_client_config = QuicClientConfig::try_from(rustls_client)
        .context("failed to build QUIC client config from rustls")?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_client_config));
    client_config.transport_config(transport_config);
    endpoint.set_default_client_config(client_config);

    let (inbound_tx, _) = broadcast::channel(512);
    Ok((endpoint, inbound_tx, pair_request_tx))
}

// ---------------------------------------------------------------------------
// TLS verifiers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PeerCertVerifier {
    expected_pubkeys: PubkeyMap,
    pair_request_tx: broadcast::Sender<PairRequest>,
    pair_request_seen: Arc<Mutex<HashMap<String, Instant>>>,
}

#[derive(Debug)]
struct PeerClientCertVerifier {
    expected_pubkeys: PubkeyMap,
    roots: Vec<DistinguishedName>,
    pair_request_tx: broadcast::Sender<PairRequest>,
    pair_request_seen: Arc<Mutex<HashMap<String, Instant>>>,
}

const PAIR_REQUEST_LOG_WINDOW: Duration = Duration::from_secs(30);

fn maybe_emit_pair_request(
    tx: &broadcast::Sender<PairRequest>,
    seen: &Mutex<HashMap<String, Instant>>,
    agent_id: &str,
    pubkey: &str,
    addr: Option<String>,
) {
    let now = Instant::now();
    let should_emit = match seen.lock() {
        Ok(mut guard) => match guard.get(agent_id) {
            Some(last) if now.duration_since(*last) < PAIR_REQUEST_LOG_WINDOW => false,
            _ => {
                guard.insert(agent_id.to_string(), now);
                true
            }
        },
        Err(_) => true,
    };

    if should_emit {
        warn!(
            agent_id = %agent_id,
            "rejected unknown peer during TLS verification; emitting pair_request event"
        );
        let _ = tx.send(PairRequest {
            agent_id: agent_id.to_string(),
            pubkey: pubkey.to_string(),
            addr,
        });
    }
}

impl ServerCertVerifier for PeerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let expected_agent_id_raw = server_name.to_str();
        let expected_agent_id = canonicalize_agent_id(&expected_agent_id_raw).ok_or_else(|| {
            rustls::Error::General(format!(
                "invalid expected server agent_id in SNI: {expected_agent_id_raw}"
            ))
        })?;

        let cert_key =
            extract_ed25519_pubkey_from_cert_der(end_entity.as_ref()).map_err(|err| {
                rustls::Error::General(format!("failed parsing server cert key: {err}"))
            })?;
        let cert_key_b64 = STANDARD.encode(cert_key);
        let derived_agent_id = derive_agent_id_from_pubkey_bytes(&cert_key);

        if derived_agent_id != expected_agent_id {
            return Err(rustls::Error::General(
                "server cert public key does not match expected agent_id".to_string(),
            ));
        }

        // std::sync required: rustls verifier callbacks are synchronous
        let expected = self
            .expected_pubkeys
            .read()
            .map_err(|_| rustls::Error::General("expected peer table lock poisoned".to_string()))?;
        if let Some(expected_pubkey_b64) = lookup_expected_pubkey(&expected, &expected_agent_id) {
            if cert_key_b64 != *expected_pubkey_b64 {
                return Err(rustls::Error::General(
                    "server cert public key mismatch against discovery data".to_string(),
                ));
            }
        } else {
            maybe_emit_pair_request(
                &self.pair_request_tx,
                &self.pair_request_seen,
                &expected_agent_id,
                &cert_key_b64,
                current_handshake_remote_addr().map(|addr| addr.to_string()),
            );
            return Err(rustls::Error::General(format!(
                "rejecting unknown server peer {expected_agent_id}: no public key on record from discovery. \
                 Add this peer to config.yaml (or run `axon connect <token>`) or ensure mDNS discovery has seen it first."
            )));
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 not supported".to_string()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl ClientCertVerifier for PeerClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.roots
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        let cert_key =
            extract_ed25519_pubkey_from_cert_der(end_entity.as_ref()).map_err(|err| {
                rustls::Error::General(format!("failed parsing client cert key: {err}"))
            })?;
        let cert_pubkey_b64 = STANDARD.encode(cert_key);
        let agent_id = derive_agent_id_from_pubkey_bytes(&cert_key);

        // std::sync required: rustls verifier callbacks are synchronous
        let expected = self
            .expected_pubkeys
            .read()
            .map_err(|_| rustls::Error::General("expected peer table lock poisoned".to_string()))?;
        if let Some(expected_pubkey_b64) = lookup_expected_pubkey(&expected, &agent_id) {
            if &cert_pubkey_b64 != expected_pubkey_b64 {
                return Err(rustls::Error::General(
                    "client cert public key does not match discovered peer key".to_string(),
                ));
            }
        } else {
            maybe_emit_pair_request(
                &self.pair_request_tx,
                &self.pair_request_seen,
                &agent_id,
                &cert_pubkey_b64,
                current_handshake_remote_addr().map(|addr| addr.to_string()),
            );
            return Err(rustls::Error::General(format!(
                "rejecting unknown client peer {agent_id}: no public key on record from discovery. \
                 Add this peer to config.yaml (or run `axon connect <token>`) or ensure mDNS discovery has seen it first."
            )));
        }

        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 not supported".to_string()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// ---------------------------------------------------------------------------
// Certificate helpers
// ---------------------------------------------------------------------------

pub fn extract_ed25519_pubkey_from_cert_der(cert_der: &[u8]) -> Result<[u8; 32]> {
    let (_remaining, cert) = parse_x509_certificate(cert_der)
        .map_err(|err| anyhow!("failed to parse certificate DER: {err}"))?;

    let key = cert.public_key().subject_public_key.data.as_ref();
    if key.len() != 32 {
        return Err(anyhow!(
            "unexpected public key length {}; expected 32 bytes Ed25519",
            key.len()
        ));
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(key);
    Ok(out)
}

fn extract_subject_dn_from_cert_der(cert_der: &[u8]) -> Result<Vec<u8>> {
    let (_remaining, cert) = parse_x509_certificate(cert_der)
        .map_err(|err| anyhow!("failed to parse certificate DER: {err}"))?;
    Ok(cert.tbs_certificate.subject.as_raw().to_vec())
}

pub(crate) fn derive_agent_id_from_pubkey_bytes(pubkey: &[u8]) -> String {
    let digest = Sha256::digest(pubkey);
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("ed25519.{hex}")
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
