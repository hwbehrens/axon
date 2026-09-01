use std::fmt;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::message::AgentId;

/// Typed failure for a persistent directory edit.
///
/// Callers (notably the IPC command handler) must distinguish "the peer is
/// unknown" — a user-facing `peer_not_found`/`peer_not_observed` — from
/// capacity and persistence failures, which are `internal_error` per
/// spec/IPC.md §5. Mapping every directory error onto the not-found classes
/// (the round-six behavior) misreports a failed disk save as a missing peer.
#[derive(Debug)]
pub enum DirectoryError {
    /// The target agent is not enrolled.
    NotEnrolled(AgentId),
    /// The candidate has no live observation to enroll from.
    NotObserved(AgentId),
    /// The enrolled-peer capacity bound was reached.
    EnrolledCapacity,
    /// The per-peer locator capacity bound was reached.
    LocatorCapacity,
    /// The local Agent ID cannot be enrolled or targeted.
    LocalAgentId(AgentId),
    /// Peer-store persistence failed (I/O error). Live memory remains the
    /// authority; the caller may retry the edit.
    Persist(anyhow::Error),
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnrolled(agent_id) => write!(f, "peer {agent_id} is not enrolled"),
            Self::NotObserved(agent_id) => {
                write!(f, "peer candidate {agent_id} is not currently observed")
            }
            Self::EnrolledCapacity => write!(f, "enrolled-peer limit reached"),
            Self::LocatorCapacity => write!(f, "per-peer locator limit reached"),
            Self::LocalAgentId(agent_id) => {
                write!(f, "cannot enroll or target the local Agent ID {agent_id}")
            }
            Self::Persist(err) => write!(f, "peer-store persistence failed: {err}"),
        }
    }
}

impl std::error::Error for DirectoryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    agent_id: AgentId,
    public_key: String,
}

impl PeerIdentity {
    pub fn from_parts(agent_id: AgentId, public_key: &str) -> Result<Self> {
        let bytes = STANDARD
            .decode(public_key.trim())
            .context("peer public key is not valid base64")?;
        let derived = AgentId::from_pubkey_bytes(&bytes)?;
        if agent_id != derived {
            bail!(
                "agent_id {} does not match public key-derived identity {}",
                agent_id,
                derived
            );
        }
        Ok(Self {
            agent_id: derived,
            public_key: STANDARD.encode(bytes),
        })
    }

    pub fn from_public_key(public_key: &str) -> Result<Self> {
        let agent_id = AgentId::from_pubkey_base64(public_key)?;
        Self::from_parts(agent_id, public_key)
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PeerLocator {
    Socket(SocketAddr),
    Host { host: Box<str>, port: u16 },
}

impl PeerLocator {
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.is_empty() {
            bail!("peer locator cannot be empty");
        }
        if let Ok(addr) = input.parse::<SocketAddr>() {
            return Ok(Self::Socket(addr));
        }
        let (host, port) = input
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("peer locator must be host:port or ip:port"))?;
        if host.is_empty() || host.chars().any(char::is_whitespace) {
            bail!("peer locator host is invalid");
        }
        let port = port
            .parse::<u16>()
            .with_context(|| format!("peer locator has invalid port '{port}'"))?;
        if port == 0 {
            bail!("peer locator port must be non-zero");
        }
        Ok(Self::Host {
            host: host.to_ascii_lowercase().into_boxed_str(),
            port,
        })
    }

    pub async fn resolve(&self) -> Result<Vec<SocketAddr>> {
        match self {
            Self::Socket(addr) => Ok(vec![*addr]),
            Self::Host { host, port } => {
                let host = host.to_string();
                let lookup_host = host.clone();
                let port = *port;
                tokio::task::spawn_blocking(move || resolve_host(&lookup_host, port))
                    .await
                    .with_context(|| format!("locator resolution task failed for {host}:{port}"))?
            }
        }
    }
}

fn resolve_host(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let mut addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve '{host}:{port}'"))?
        .collect();
    addresses.sort_by_key(|addr| (!addr.is_ipv4(), *addr));
    addresses.dedup();
    if addresses.is_empty() {
        bail!("resolution returned no addresses for '{host}:{port}'");
    }
    Ok(addresses)
}

impl fmt::Display for PeerLocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(addr) => write!(f, "{addr}"),
            Self::Host { host, port } => write!(f, "{host}:{port}"),
        }
    }
}

impl Serialize for PeerLocator {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PeerLocator {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObservationId(Box<str>);

impl ObservationId {
    pub fn new(value: impl Into<Box<str>>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 512 {
            bail!("observation id must contain 1 to 512 bytes");
        }
        Ok(Self(value))
    }
}

impl fmt::Display for ObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationSource {
    Mdns,
    Handshake,
}

#[derive(Debug, Clone)]
pub struct PeerObservation {
    pub id: ObservationId,
    pub identity: PeerIdentity,
    pub endpoint: Option<SocketAddr>,
    pub display_name: Option<Box<str>>,
    pub source: ObservationSource,
    pub observed_at: Instant,
}

impl PeerObservation {
    pub fn new(
        id: ObservationId,
        agent_id: AgentId,
        public_key: &str,
        endpoint: Option<SocketAddr>,
        display_name: Option<Box<str>>,
        source: ObservationSource,
    ) -> Result<Self> {
        Ok(Self {
            id,
            identity: PeerIdentity::from_parts(agent_id, public_key)?,
            endpoint,
            display_name,
            source,
            observed_at: Instant::now(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerTrust {
    Candidate,
    Enrolled,
}

#[derive(Debug, Clone)]
pub struct PeerView {
    pub identity: PeerIdentity,
    pub trust: PeerTrust,
    pub configured_locators: Vec<PeerLocator>,
    pub observed_endpoints: Vec<SocketAddr>,
    pub display_name: Option<Box<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialTarget {
    Configured(PeerLocator),
    Observed(SocketAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveOutcome {
    IgnoredSelf,
    CandidateAdded,
    CandidateRefreshed,
    EnrolledPeerRefreshed,
    IdentityConflict,
    LocatorConflict,
    CapacityReached,
}
