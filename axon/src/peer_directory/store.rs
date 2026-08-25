use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{MAX_ENROLLED_PEERS, MAX_LOCATORS_PER_PEER, PeerIdentity, PeerLocator};
use crate::message::AgentId;

const PEER_STORE_VERSION: u32 = 1;

/// Hard cap on peer-store file size. The logical bounds (`MAX_ENROLLED_PEERS`,
/// `MAX_LOCATORS_PER_PEER`) bound a well-formed store to roughly 200 KB, so
/// 1 MiB leaves generous encoding headroom while refusing unbounded reads:
/// `fs::read` on hostile or corrupted input must not allocate arbitrarily.
pub const MAX_PEER_STORE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PeerStore {
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPeer {
    pub agent_id: AgentId,
    /// Canonical field name per spec/SPEC.md "Peer Store Format". The alias
    /// accepts the pre-release `public_key` spelling on read only.
    #[serde(rename = "pubkey", alias = "public_key")]
    pub public_key: String,
    pub locators: Vec<PeerLocator>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerStoreDocument {
    version: u32,
    peers: Vec<StoredPeer>,
}

impl PeerStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn validate(&self) -> Result<usize> {
        self.load().await.map(|peers| peers.len())
    }

    pub(crate) async fn load(&self) -> Result<Vec<StoredPeer>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || load_sync(&path))
            .await
            .context("peer-store load task failed")?
    }

    /// Decode and validate peer-store bytes without touching the filesystem.
    ///
    /// This is the single validated entrypoint for untrusted peer-store
    /// content: file loads and the fuzz harness share it, so no caller can
    /// bypass version, bound, identity-binding, or duplicate checks.
    pub fn decode(data: &[u8]) -> Result<Vec<StoredPeer>> {
        // The shared entrypoint for untrusted content: refuse oversized
        // input before parsing so no caller (file load, fuzz harness) can
        // force large allocations.
        if data.len() > MAX_PEER_STORE_BYTES {
            bail!(
                "peer store content is {} bytes; maximum is {MAX_PEER_STORE_BYTES}",
                data.len()
            );
        }
        let document: PeerStoreDocument = serde_json::from_slice(data)
            .with_context(|| "failed to parse peer store".to_string())?;
        if document.version != PEER_STORE_VERSION {
            bail!(
                "unsupported peer-store version {}; expected {PEER_STORE_VERSION}",
                document.version
            );
        }
        validate_peers(&document.peers)?;
        Ok(document.peers)
    }

    pub(crate) async fn save(&self, peers: Vec<StoredPeer>) -> Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || save_sync(&path, peers))
            .await
            .context("peer-store save task failed")?
    }
}

fn load_sync(path: &Path) -> Result<Vec<StoredPeer>> {
    // symlink_metadata inspects the path itself: a symlink pointing anywhere
    // (even inside the state root) is refused rather than followed.
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read peer store: {}", path.display()));
        }
    };
    if !meta.file_type().is_file() {
        bail!(
            "refusing to load non-regular peer-store file: {}",
            path.display()
        );
    }
    if meta.len() as usize > MAX_PEER_STORE_BYTES {
        bail!(
            "peer store is {} bytes; maximum is {MAX_PEER_STORE_BYTES}",
            meta.len()
        );
    }
    // Re-check the size at read time via `take`: the file may have grown
    // between the metadata check and the open.
    let file = File::open(path)
        .with_context(|| format!("failed to open peer store: {}", path.display()))?;
    let mut data = Vec::new();
    file.take(MAX_PEER_STORE_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("failed to read peer store: {}", path.display()))?;
    if data.len() > MAX_PEER_STORE_BYTES {
        bail!("peer store exceeds {MAX_PEER_STORE_BYTES} byte limit",);
    }
    PeerStore::decode(&data).with_context(|| format!("in peer store {}", path.display()))
}

fn save_sync(path: &Path, peers: Vec<StoredPeer>) -> Result<()> {
    validate_peers(&peers)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("peer-store path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create peer-store directory: {}",
            parent.display()
        )
    })?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "failed to set peer-store directory permissions: {}",
            parent.display()
        )
    })?;

    let document = PeerStoreDocument {
        version: PEER_STORE_VERSION,
        peers,
    };
    let mut data = serde_json::to_vec_pretty(&document).context("failed to encode peer store")?;
    data.push(b'\n');
    // Enforce the same cap `load_sync` enforces on read: a save that
    // produced an over-cap store would be refused at the next restart,
    // bricking the state root. Fail here, before anything is written.
    if data.len() > MAX_PEER_STORE_BYTES {
        bail!(
            "serialized peer store is {} bytes; maximum is {MAX_PEER_STORE_BYTES}",
            data.len()
        );
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("peers.json");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let result = write_and_replace(path, &temp_path, parent, &data);
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn write_and_replace(path: &Path, temp_path: &Path, parent: &Path, data: &[u8]) -> Result<()> {
    let mut temp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp_path)
        .with_context(|| {
            format!(
                "failed to create temporary peer store: {}",
                temp_path.display()
            )
        })?;
    temp.write_all(data).with_context(|| {
        format!(
            "failed to write temporary peer store: {}",
            temp_path.display()
        )
    })?;
    temp.sync_all().with_context(|| {
        format!(
            "failed to sync temporary peer store: {}",
            temp_path.display()
        )
    })?;
    drop(temp);

    fs::rename(temp_path, path).with_context(|| {
        format!(
            "failed to atomically replace peer store {} with {}",
            path.display(),
            temp_path.display()
        )
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync peer-store directory: {}", parent.display()))?;
    Ok(())
}

fn validate_bounds(peers: &[StoredPeer]) -> Result<()> {
    if peers.len() > MAX_ENROLLED_PEERS {
        bail!(
            "peer store contains {} peers; maximum is {MAX_ENROLLED_PEERS}",
            peers.len()
        );
    }
    for peer in peers {
        if peer.locators.len() > MAX_LOCATORS_PER_PEER {
            bail!(
                "peer {} contains {} locators; maximum is {MAX_LOCATORS_PER_PEER}",
                peer.agent_id,
                peer.locators.len()
            );
        }
    }
    Ok(())
}

fn validate_peers(peers: &[StoredPeer]) -> Result<()> {
    validate_bounds(peers)?;
    let mut identities = std::collections::BTreeSet::new();
    for peer in peers {
        PeerIdentity::from_parts(peer.agent_id.clone(), &peer.public_key)?;
        if !identities.insert(peer.agent_id.clone()) {
            bail!("peer store contains duplicate Agent ID {}", peer.agent_id);
        }
        let unique_locators: std::collections::BTreeSet<_> = peer.locators.iter().collect();
        if unique_locators.len() != peer.locators.len() {
            bail!("peer {} contains duplicate locators", peer.agent_id);
        }
    }
    Ok(())
}
