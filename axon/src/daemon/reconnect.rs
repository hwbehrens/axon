use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::warn;

use crate::peer_table::{ConnectionStatus, PeerTable};
use crate::transport::QuicTransport;

#[derive(Debug, Clone)]
pub(crate) struct ReconnectState {
    pub(crate) next_attempt_at: Instant,
    pub(crate) current_backoff: Duration,
}

impl ReconnectState {
    pub(crate) fn immediate(now: Instant) -> Self {
        Self {
            next_attempt_at: now,
            current_backoff: Duration::from_secs(1),
        }
    }

    pub(crate) fn schedule_failure(&mut self, now: Instant, max_backoff: Duration) -> Duration {
        let wait = self.current_backoff;
        self.next_attempt_at = now + wait;
        self.current_backoff = std::cmp::min(wait.saturating_mul(2), max_backoff);
        wait
    }
}

pub(crate) async fn attempt_reconnects(
    peer_table: &PeerTable,
    transport: &QuicTransport,
    local_agent_id: &str,
    reconnect_state: &mut HashMap<String, ReconnectState>,
    max_backoff: Duration,
) {
    let now = Instant::now();

    for peer in peer_table.list().await {
        let mut status = peer.status;
        if status == ConnectionStatus::Connected && !transport.has_connection(&peer.agent_id).await
        {
            peer_table.set_disconnected(&peer.agent_id).await;
            status = ConnectionStatus::Disconnected;
        }

        if local_agent_id < peer.agent_id.as_str() && status != ConnectionStatus::Connected {
            reconnect_state
                .entry(peer.agent_id)
                .or_insert_with(|| ReconnectState::immediate(now));
        }
    }

    let attempt_ids: Vec<String> = reconnect_state
        .iter()
        .filter_map(|(id, state)| {
            if state.next_attempt_at <= now {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    for agent_id in attempt_ids {
        let Some(peer) = peer_table.get(&agent_id).await else {
            reconnect_state.remove(&agent_id);
            continue;
        };

        if peer.status == ConnectionStatus::Connected && transport.has_connection(&agent_id).await {
            reconnect_state.remove(&agent_id);
            continue;
        }
        if peer.status == ConnectionStatus::Connected {
            peer_table.set_disconnected(&agent_id).await;
        }

        peer_table
            .set_status(&agent_id, ConnectionStatus::Connecting)
            .await;
        match transport.ensure_connection(&peer).await {
            Ok(conn) => {
                let rtt = conn.rtt().as_secs_f64() * 1000.0;
                peer_table.set_connected(&agent_id, Some(rtt)).await;
                reconnect_state.remove(&agent_id);
            }
            Err(err) => {
                peer_table.set_disconnected(&agent_id).await;
                if let Some(state) = reconnect_state.get_mut(&agent_id) {
                    let wait = state.schedule_failure(now, max_backoff);
                    warn!(
                        peer_id = %agent_id,
                        error = %err,
                        next_attempt_in_secs = wait.as_secs(),
                        "failed reconnect; scheduling backoff retry"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;
