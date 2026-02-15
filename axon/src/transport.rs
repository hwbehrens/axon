use anyhow::{Context, Result};
use quinn::{Endpoint, ServerConfig, ClientConfig, Connection, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use crate::identity::Identity;
use crate::message::{Envelope, HelloPayload, HelloResponsePayload};
use serde_json::json;

pub struct Transport {
    pub endpoint: Endpoint,
    pub agent_id: String,
}

impl Transport {
    pub fn new(identity: &Identity, port: u16) -> Result<Self> {
        let cert = identity.self_signed_cert()?;
        let cert_der = cert.serialize_der()?;
        let priv_key_der = cert.serialize_private_key_der();

        let server_config = ServerConfig::with_single_cert(
            vec![rustls::Certificate(cert_der.clone())],
            rustls::PrivateKey(priv_key_der),
        )?;

        let mut endpoint = Endpoint::server(server_config, SocketAddr::from(([0, 0, 0, 0], port)))?;
        
        let client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification {}))
            .with_no_client_auth();
        
        endpoint.set_default_client_config(ClientConfig::new(Arc::new(client_crypto)));

        Ok(Self {
            endpoint,
            agent_id: identity.agent_id(),
        })
    }

    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection> {
        let conn = self.endpoint.connect(addr, "localhost")?.await?;
        Ok(conn)
    }

    pub async fn accept(&self) -> Option<quinn::Connecting> {
        self.endpoint.accept().await
    }
}

pub async fn send_envelope(send: &mut SendStream, envelope: &Envelope) -> Result<()> {
    let bytes = serde_json::to_vec(envelope)?;
    let len = bytes.len() as u32;
    send.write_all(&len.to_be_bytes()).await?;
    send.write_all(&bytes).await?;
    send.finish().await?;
    Ok(())
}

pub async fn recv_envelope(recv: &mut RecvStream) -> Result<Envelope> {
    let mut len_bytes = [0u8; 4];
    recv.read_exact(&mut len_bytes).await?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    
    let envelope: Envelope = serde_json::from_slice(&buf)?;
    Ok(envelope)
}

struct SkipServerVerification {}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
