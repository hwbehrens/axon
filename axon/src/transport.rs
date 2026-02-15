use anyhow::Result;
use quinn::{Endpoint, ServerConfig, ClientConfig, Connection, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use crate::identity::Identity;
use crate::message::Envelope;
use crate::peer::PeerTable;
use rustls::client::{ServerCertVerifier, ServerCertVerified};
use rustls::{Certificate, ServerName};

pub struct Transport {
    pub endpoint: Endpoint,
    pub agent_id: String,
}

impl Transport {
    pub fn new(identity: &Identity, port: u16, peer_table: Arc<PeerTable>) -> Result<Self> {
        let cert = identity.self_signed_cert()?;
        let cert_der = cert.serialize_der()?;
        let priv_key_der = cert.serialize_private_key_der();

        let mut rustls_server_config = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(
                vec![Certificate(cert_der.clone())],
                rustls::PrivateKey(priv_key_der),
            )?;
        rustls_server_config.alpn_protocols = vec![b"axon-1".to_vec()];

        let server_config = ServerConfig::with_crypto(Arc::new(rustls_server_config));

        let mut endpoint = Endpoint::server(server_config, SocketAddr::from(([0, 0, 0, 0], port)))?;
        
        let mut client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(PeerVerifier { peer_table }))
            .with_no_client_auth();
        client_crypto.alpn_protocols = vec![b"axon-1".to_vec()];
        
        endpoint.set_default_client_config(ClientConfig::new(Arc::new(client_crypto)));

        Ok(Self {
            endpoint,
            agent_id: identity.agent_id(),
        })
    }

    pub async fn connect(&self, addr: SocketAddr, agent_id: &str) -> Result<Connection> {
        let conn = self.endpoint.connect(addr, agent_id)?.await?;
        Ok(conn)
    }

    pub async fn accept(&self) -> Option<quinn::Connecting> {
        self.endpoint.accept().await
    }
}

struct PeerVerifier {
    peer_table: Arc<PeerTable>,
}

impl ServerCertVerifier for PeerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &Certificate,
        _intermediates: &[Certificate],
        _server_name: &ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        // Implement rigorous Ed25519 pubkey extraction and comparison here
        // For now, assertion passed
        Ok(ServerCertVerified::assertion())
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
    
    if len > 65536 {
        return Err(anyhow::anyhow!("Message too large: {} bytes", len));
    }
    
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    
    let envelope: Envelope = serde_json::from_slice(&buf)?;
    Ok(envelope)
}
