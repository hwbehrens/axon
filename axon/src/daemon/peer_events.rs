use std::collections::HashMap;
use std::time::Instant;

use tracing::warn;

use super::reconnect::ReconnectState;
use crate::discovery::PeerEvent;
use crate::message::AgentId;
use crate::peer_table::{PeerSource, PeerTable};

pub(crate) async fn handle_peer_event(
    event: PeerEvent,
    peer_table: &PeerTable,
    reconnect_state: &mut HashMap<AgentId, ReconnectState>,
) {
    let now = Instant::now();

    match event {
        PeerEvent::Discovered {
            agent_id,
            addr,
            pubkey,
        } => {
            if let Some(existing) = peer_table.get(&agent_id).await {
                if matches!(existing.source, PeerSource::Static | PeerSource::Cached)
                    && existing.pubkey != pubkey
                {
                    warn!(
                        peer_id = %agent_id,
                        source = ?existing.source,
                        "ignoring discovered pubkey change for pinned peer"
                    );
                    reconnect_state
                        .entry(agent_id)
                        .or_insert_with(|| ReconnectState::immediate(now));
                    return;
                }

                if existing.source == PeerSource::Static {
                    peer_table
                        .refresh_static_addr(agent_id.as_str(), addr, &pubkey)
                        .await;
                    reconnect_state.insert(agent_id, ReconnectState::immediate(now));
                    return;
                }
            }

            peer_table
                .upsert_discovered(agent_id.clone(), addr, pubkey)
                .await;

            reconnect_state.insert(agent_id, ReconnectState::immediate(now));
        }
        PeerEvent::Lost { agent_id } => {
            if matches!(
                peer_table.get(&agent_id).await.map(|peer| peer.source),
                Some(PeerSource::Discovered)
            ) {
                peer_table.remove(&agent_id).await;
            }
            reconnect_state.remove(&agent_id);
        }
    }
}

#[cfg(test)]
#[path = "peer_events_tests.rs"]
mod tests;
