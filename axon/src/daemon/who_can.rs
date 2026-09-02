//! `who_can` — a derived, cached view of connected peers' capabilities.
//!
//! On each query, manifests for connected enrolled peers are pulled via
//! `describe` exchanges only when missing or stale (TTL-gated); fresh cache
//! entries answer without network traffic. The cache is runtime-only and
//! advisory: it grants no authority and introduces no durable state.
//! Peers that fail a pull are named in the reply — partial results are never
//! silently incomplete.

use std::time::Duration;

use serde_json::json;
use tracing::{debug, warn};

use crate::ipc::{DaemonReply, ServiceMatch, ServiceSummary};
use crate::manifest::Manifest;
use crate::message::{AgentId, Envelope, MessageKind};
use crate::peer_directory::PeerTrust;

use super::DaemonContext;

/// Whole-exchange deadline for one `describe` capability pull.
pub(crate) const WHO_CAN_PULL_TIMEOUT: Duration = Duration::from_secs(5);

/// Cached manifests older than this are re-pulled on the next `who_can`.
pub(crate) const WHO_CAN_CACHE_TTL: Duration = Duration::from_secs(60);

pub(super) async fn handle(ctx: &DaemonContext, query: Option<String>) -> DaemonReply {
    // Backpressure: one `who_can` computation runs at a time. Concurrent
    // queries would stack unbounded concurrent pulls; queued queries instead
    // wait here, then re-read the now-fresh cache cheaply.
    let _gate = ctx.who_can_gate.lock().await;

    // Capability data is pulled over live TLS sessions, so only connected
    // enrolled peers are in scope; candidates cannot be messaged at all.
    let mut connected: Vec<AgentId> = Vec::new();
    for peer in ctx.directory.list().await {
        if peer.trust != PeerTrust::Enrolled {
            continue;
        }
        let agent_id = peer.identity.agent_id().clone();
        if ctx.transport.has_connection(&agent_id).await {
            connected.push(agent_id);
        }
    }
    connected.sort();

    // Connection-scoped cache: entries for peers no longer connected (or
    // revoked) are evicted here, so advisory `services` summaries can never
    // outlive the connection that produced them.
    ctx.manifest_cache.retain_connected(&connected).await;

    // Refresh stale or missing entries concurrently; fresh entries answer
    // from cache without waking the network.
    let mut pulls = tokio::task::JoinSet::new();
    for agent_id in &connected {
        if ctx
            .manifest_cache
            .fresh(agent_id, WHO_CAN_CACHE_TTL)
            .await
            .is_none()
        {
            let ctx = ctx.clone();
            let agent_id = agent_id.clone();
            pulls.spawn(async move { pull_manifest(&ctx, &agent_id).await });
        }
    }
    let mut unreachable: Vec<String> = Vec::new();
    while let Some(joined) = pulls.join_next().await {
        match joined {
            Ok(PullOutcome::Manifest(agent_id, manifest)) => {
                // A pull can outlive its connection snapshot; only cache
                // manifests for peers still connected right now.
                if ctx.transport.has_connection(&agent_id).await {
                    ctx.manifest_cache.insert(agent_id, manifest).await;
                } else {
                    debug!(peer = %agent_id, "discarding capability pull for disconnected peer");
                }
            }
            Ok(PullOutcome::Declined(agent_id)) => {
                debug!(peer = %agent_id, "peer published no capability manifest");
            }
            Ok(PullOutcome::Unreachable(agent_id)) => unreachable.push(agent_id.to_string()),
            Err(err) if err.is_panic() => warn!(error = %err, "capability pull task panicked"),
            Err(_) => {}
        }
    }

    let mut matches: Vec<ServiceMatch> = Vec::new();
    for agent_id in &connected {
        let Some(manifest) = ctx.manifest_cache.fresh(agent_id, WHO_CAN_CACHE_TTL).await else {
            continue;
        };
        let services = match_services(query.as_deref(), &manifest);
        if services.is_empty() {
            continue;
        }
        matches.push(ServiceMatch {
            agent_id: agent_id.to_string(),
            services,
        });
    }
    matches.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    unreachable.sort();
    unreachable.dedup();

    DaemonReply::WhoCan {
        ok: true,
        matches,
        unreachable,
        req_id: None,
    }
}

enum PullOutcome {
    Manifest(AgentId, Manifest),
    /// The peer answered but published no usable manifest (e.g. an explicit
    /// `no_manifest` error). Reported as "no services", not unreachable.
    Declined(AgentId),
    Unreachable(AgentId),
}

async fn pull_manifest(ctx: &DaemonContext, agent_id: &AgentId) -> PullOutcome {
    let envelope = Envelope::new(
        ctx.local_agent_id.clone(),
        agent_id.clone(),
        MessageKind::Describe,
        json!({}),
    );
    match ctx
        .transport
        .send_to(&ctx.directory, agent_id, envelope, WHO_CAN_PULL_TIMEOUT)
        .await
    {
        Ok(Some(response)) => match response.kind {
            MessageKind::Response => match response.payload_as::<Manifest>() {
                Ok(manifest) => PullOutcome::Manifest(agent_id.clone(), manifest),
                // A `response` whose payload is not a valid manifest is a
                // failed capability pull, not a declined one.
                Err(_) => PullOutcome::Unreachable(agent_id.clone()),
            },
            MessageKind::Error => {
                // Expected, explicit declines: nothing published, or an older
                // peer that cannot apply bidirectional `describe` semantics
                // (spec/MESSAGE_TYPES.md forward-compatibility rule).
                let code = response.payload_value().ok().and_then(|payload| {
                    payload
                        .get("code")
                        .and_then(|code| code.as_str())
                        .map(String::from)
                });
                match code.as_deref() {
                    Some("no_manifest") | Some("unsupported_kind") => {
                        PullOutcome::Declined(agent_id.clone())
                    }
                    _ => PullOutcome::Unreachable(agent_id.clone()),
                }
            }
            _ => PullOutcome::Unreachable(agent_id.clone()),
        },
        Ok(None) | Err(_) => PullOutcome::Unreachable(agent_id.clone()),
    }
}

/// Case-insensitive substring match over service id and description. The
/// query is trimmed and lowercased here; absent or whitespace-only queries
/// list every service.
fn match_services(query: Option<&str>, manifest: &Manifest) -> Vec<ServiceSummary> {
    // Unicode-aware lowercasing on both sides: manifest ids and descriptions
    // may contain non-ASCII text (only whitespace/control characters are
    // excluded), so ASCII-only folding would miss legitimate matches.
    let needle = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    manifest
        .services
        .iter()
        .filter(|service| match &needle {
            None => true,
            Some(needle) => {
                service.id.to_lowercase().contains(needle.as_str())
                    || service.description.to_lowercase().contains(needle.as_str())
            }
        })
        .map(|service| ServiceSummary {
            id: service.id.clone(),
            description: service.description.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "who_can_tests.rs"]
mod tests;
