//! Capability manifests — self-described service catalogs exchanged via the
//! `describe` message kind.
//!
//! A manifest is a *claim* published by the local IPC application handler at
//! `serve` time. The daemon caches it and answers inbound `describe` requests
//! from that cache without waking the handler. Manifests never affect TLS
//! trust, peer pinning, or enrollment; serving one is opt-in and absence is
//! reported explicitly (`no_manifest`), never silently.
//!
//! See `spec/MESSAGE_TYPES.md` §describe and `spec/WIRE_FORMAT.md` for the
//! normative schema and bounds.

mod cache;
mod types;

pub use cache::ManifestCache;
pub use types::{MAX_MANIFEST_BYTES, MAX_SERVICES, Manifest, ServiceEntry};
