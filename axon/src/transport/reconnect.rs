use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::message::AgentId;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
pub(crate) struct AttemptTicket {
    pub(crate) version: u64,
}

#[derive(Debug)]
struct AttemptState {
    version: u64,
    next_attempt: Instant,
    backoff: Duration,
    in_flight: bool,
}

#[derive(Debug, Default)]
struct ReconnectState {
    next_version: u64,
    attempts: HashMap<AgentId, AttemptState>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReconnectBook {
    state: Arc<Mutex<ReconnectState>>,
}

impl ReconnectBook {
    pub(crate) async fn claim(&self, peer: AgentId, now: Instant) -> Option<AttemptTicket> {
        let mut state = self.state.lock().await;
        if !state.attempts.contains_key(&peer) {
            state.next_version = state.next_version.wrapping_add(1).max(1);
            let version = state.next_version;
            state.attempts.insert(
                peer.clone(),
                AttemptState {
                    version,
                    next_attempt: now,
                    backoff: INITIAL_BACKOFF,
                    in_flight: false,
                },
            );
        }
        let attempt = state.attempts.get_mut(&peer)?;
        if attempt.in_flight || attempt.next_attempt > now {
            return None;
        }
        attempt.in_flight = true;
        Some(AttemptTicket {
            version: attempt.version,
        })
    }

    pub(crate) async fn succeeded(&self, peer: &AgentId, ticket: AttemptTicket) {
        let mut state = self.state.lock().await;
        if state
            .attempts
            .get(peer)
            .is_some_and(|attempt| attempt.version == ticket.version)
        {
            state.attempts.remove(peer);
        }
    }

    pub(crate) async fn failed(
        &self,
        peer: &AgentId,
        ticket: AttemptTicket,
        now: Instant,
    ) -> Option<Duration> {
        let mut state = self.state.lock().await;
        let attempt = state.attempts.get_mut(peer)?;
        if attempt.version != ticket.version {
            return None;
        }
        let wait = attempt.backoff;
        attempt.next_attempt = now + wait;
        attempt.backoff = attempt.backoff.saturating_mul(2).min(MAX_BACKOFF);
        attempt.in_flight = false;
        Some(wait)
    }

    /// Release an attempt whose task ended without a dial outcome — shutdown
    /// or per-peer revocation cancelled it mid-flight. The entry keeps its
    /// backoff state but becomes claimable again immediately: without this,
    /// a cancelled attempt would stay `in_flight` forever and maintenance
    /// could never claim another attempt for the peer after re-enrollment.
    pub(crate) async fn abandoned(&self, peer: &AgentId, ticket: AttemptTicket) {
        let mut state = self.state.lock().await;
        if let Some(attempt) = state.attempts.get_mut(peer)
            && attempt.version == ticket.version
        {
            attempt.in_flight = false;
        }
    }

    pub(crate) async fn retain(&self, enrolled: &HashSet<AgentId>) {
        self.state
            .lock()
            .await
            .attempts
            .retain(|peer, _| enrolled.contains(peer));
    }
}

#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;
