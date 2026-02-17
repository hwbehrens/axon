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
    local_agent_id: &AgentId,
    reconnect_state: &mut HashMap<AgentId, ReconnectState>,
) {
    let now = Instant::now();

    match event {
        PeerEvent::Discovered {
            agent_id,
            addr,
            pubkey,
        } => {
            if let Some(existing) = peer_table.get(&agent_id).await
                && matches!(existing.source, PeerSource::Static | PeerSource::Cached)
                && existing.pubkey != pubkey
            {
                warn!(
                    peer_id = %agent_id,
                    source = ?existing.source,
                    "ignoring discovered pubkey change for pinned peer"
                );
                if local_agent_id < &agent_id {
                    reconnect_state
                        .entry(agent_id)
                        .or_insert_with(|| ReconnectState::immediate(now));
                }
                return;
            }

            peer_table
                .upsert_discovered(agent_id.clone(), addr, pubkey)
                .await;

            if local_agent_id < &agent_id {
                reconnect_state.insert(agent_id, ReconnectState::immediate(now));
            }
        }
        PeerEvent::Lost { agent_id } => {
            peer_table.set_disconnected(&agent_id).await;
            reconnect_state.remove(&agent_id);
        }
    }
}
