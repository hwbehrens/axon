use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::warn;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::message::AgentId;
use crate::peer_table::{ConnectionStatus, PeerTable};
use crate::transport::QuicTransport;

/// Timeout for a single reconnection attempt (QUIC handshake to peer).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(crate) struct ReconnectState {
    pub(crate) next_attempt_at: Instant,
    pub(crate) current_backoff: Duration,
    pub(crate) in_flight: bool,
}

impl ReconnectState {
    pub(crate) fn immediate(now: Instant) -> Self {
        Self {
            next_attempt_at: now,
            current_backoff: Duration::from_secs(1),
            in_flight: false,
        }
    }

    pub(crate) fn schedule_failure(&mut self, now: Instant, max_backoff: Duration) -> Duration {
        let wait = self.current_backoff;
        self.next_attempt_at = now + wait;
        self.current_backoff = std::cmp::min(wait.saturating_mul(2), max_backoff);
        self.in_flight = false;
        wait
    }
}

/// Result reported back from a spawned reconnect task to the main event loop.
pub(crate) struct ReconnectOutcome {
    pub(crate) agent_id: AgentId,
    pub(crate) result: Result<f64, anyhow::Error>,
}

/// Create the channel pair for reconnect outcomes.
pub(crate) fn reconnect_channel() -> (
    mpsc::Sender<ReconnectOutcome>,
    mpsc::Receiver<ReconnectOutcome>,
) {
    mpsc::channel(64)
}

/// Handle a reconnect outcome reported by a spawned task.
/// Updates peer_table and reconnect_state accordingly.
pub(crate) async fn handle_reconnect_outcome(
    outcome: ReconnectOutcome,
    peer_table: &PeerTable,
    reconnect_state: &mut HashMap<AgentId, ReconnectState>,
    max_backoff: Duration,
) {
    match outcome.result {
        Ok(rtt_ms) => {
            peer_table
                .set_connected(&outcome.agent_id, Some(rtt_ms))
                .await;
            reconnect_state.remove(&outcome.agent_id);
        }
        Err(err) => {
            peer_table.set_disconnected(&outcome.agent_id).await;
            if let Some(state) = reconnect_state.get_mut(&outcome.agent_id) {
                let wait = state.schedule_failure(Instant::now(), max_backoff);
                warn!(
                    peer_id = %outcome.agent_id,
                    error = %err,
                    next_attempt_in_secs = wait.as_secs(),
                    "reconnect attempt failed; scheduling backoff retry"
                );
            }
        }
    }
}

/// Scan peers and spawn reconnect tasks for those that are due.
///
/// Connection attempts run in spawned tasks so they don't block the
/// main event loop.  Results are reported back via `outcome_tx`.
pub(crate) async fn attempt_reconnects(
    peer_table: &PeerTable,
    transport: &QuicTransport,
    reconnect_state: &mut HashMap<AgentId, ReconnectState>,
    cancel: &CancellationToken,
    outcome_tx: &mpsc::Sender<ReconnectOutcome>,
) {
    let now = Instant::now();

    for peer in peer_table.list().await {
        let mut status = peer.status;
        let has_conn = transport.has_connection(&peer.agent_id).await;

        if status == ConnectionStatus::Connected && !has_conn {
            peer_table.set_disconnected(&peer.agent_id).await;
            status = ConnectionStatus::Disconnected;
        } else if status != ConnectionStatus::Connected && has_conn {
            peer_table.set_connected(&peer.agent_id, None).await;
            reconnect_state.remove(&peer.agent_id);
            continue;
        }

        if status != ConnectionStatus::Connected {
            reconnect_state
                .entry(peer.agent_id)
                .or_insert_with(|| ReconnectState::immediate(now));
        }
    }

    let attempt_ids: Vec<AgentId> = reconnect_state
        .iter()
        .filter_map(|(id, state)| {
            if state.next_attempt_at <= now && !state.in_flight {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    for agent_id in attempt_ids {
        if cancel.is_cancelled() {
            break;
        }

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

        // Mark in-flight so we don't spawn duplicate attempts.
        if let Some(state) = reconnect_state.get_mut(&agent_id) {
            state.in_flight = true;
        }

        peer_table
            .set_status(&agent_id, ConnectionStatus::Connecting)
            .await;

        let transport = transport.clone();
        let cancel = cancel.clone();
        let outcome_tx = outcome_tx.clone();
        tokio::spawn(async move {
            let connect_result = tokio::select! {
                _ = cancel.cancelled() => Err(anyhow::anyhow!("cancelled")),
                result = tokio::time::timeout(
                    CONNECT_TIMEOUT,
                    transport.ensure_connection(&peer),
                ) => match result {
                    Ok(inner) => inner,
                    Err(_elapsed) => Err(anyhow::anyhow!("connection attempt timed out")),
                },
            };

            let result = connect_result.map(|conn| conn.rtt().as_secs_f64() * 1000.0);
            let _ = outcome_tx.send(ReconnectOutcome { agent_id, result }).await;
        });
    }
}

#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;
