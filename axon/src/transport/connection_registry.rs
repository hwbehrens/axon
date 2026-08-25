use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::message::AgentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Inbound,
    Outbound,
}

#[derive(Clone)]
struct ConnectionSlot {
    generation: u64,
    direction: Direction,
    connection: quinn::Connection,
}

#[derive(Default)]
struct RegistryState {
    slots: HashMap<AgentId, ConnectionSlot>,
    generations: HashMap<AgentId, u64>,
}

#[derive(Clone)]
pub(crate) struct ConnectionRegistry {
    local_agent_id: AgentId,
    state: Arc<RwLock<RegistryState>>,
}

pub(crate) enum Admission {
    Accepted {
        generation: u64,
    },
    Existing(quinn::Connection),
    /// The admission gate refused the connection (e.g., the peer was revoked
    /// while its handshake was in flight). The connection has been closed;
    /// no slot was created or changed.
    Rejected,
}

impl ConnectionRegistry {
    pub(crate) fn new(local_agent_id: AgentId) -> Self {
        Self {
            local_agent_id,
            state: Arc::new(RwLock::new(RegistryState::default())),
        }
    }

    pub(crate) async fn current(&self, peer: &AgentId) -> Option<quinn::Connection> {
        let mut state = self.state.write().await;
        if state
            .slots
            .get(peer)
            .is_some_and(|slot| slot.connection.close_reason().is_some())
        {
            state.slots.remove(peer);
            *state.generations.entry(peer.clone()).or_default() += 1;
        }
        state.slots.get(peer).map(|slot| slot.connection.clone())
    }

    /// Admission with an authorization gate consulted atomically against
    /// slot installation.
    ///
    /// The gate future runs while the registry's state lock is held, which
    /// linearizes it against every mutation that closes slots through this
    /// registry (notably revocation's `close_peer`): either the gate observes
    /// the authority change and refuses, or the subsequent slot-closing
    /// mutation lands after installation and tears the fresh slot down.
    /// A check performed before acquiring this lock would admit handshakes
    /// that raced a revocation.
    ///
    /// Lock ordering: `state` first, then whatever `gate` acquires. Gates
    /// MUST NOT acquire a lock whose holders acquire the registry state lock.
    pub(crate) async fn admit_gated<F, Fut>(
        &self,
        peer: AgentId,
        connection: quinn::Connection,
        direction: Direction,
        gate: F,
    ) -> Admission
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut state = self.state.write().await;
        if !gate().await {
            drop(state);
            connection.close(0u32.into(), b"peer not admitted");
            return Admission::Rejected;
        }
        if state
            .slots
            .get(&peer)
            .is_some_and(|slot| slot.connection.close_reason().is_some())
        {
            state.slots.remove(&peer);
            *state.generations.entry(peer.clone()).or_default() += 1;
        }
        let generation = *state.generations.entry(peer.clone()).or_default();
        if let Some(incumbent) = state.slots.get(&peer) {
            if incumbent.connection.stable_id() == connection.stable_id() {
                return Admission::Existing(incumbent.connection.clone());
            }
            let preferred = if self.local_agent_id < peer {
                Direction::Outbound
            } else {
                Direction::Inbound
            };
            if incumbent.direction == preferred || direction != preferred {
                connection.close(0u32.into(), b"duplicate connection");
                return Admission::Existing(incumbent.connection.clone());
            }
        }
        let replaced = state.slots.insert(
            peer,
            ConnectionSlot {
                generation,
                direction,
                connection: connection.clone(),
            },
        );
        drop(state);
        if let Some(replaced) = replaced {
            replaced
                .connection
                .close(0u32.into(), b"preferred connection selected");
        }
        Admission::Accepted { generation }
    }

    pub(crate) async fn release_if_current(
        &self,
        peer: &AgentId,
        generation: u64,
        stable_id: usize,
    ) {
        let mut state = self.state.write().await;
        let is_current = state.slots.get(peer).is_some_and(|slot| {
            slot.generation == generation && slot.connection.stable_id() == stable_id
        });
        if is_current {
            state.slots.remove(peer);
            *state.generations.entry(peer.clone()).or_default() += 1;
        }
    }

    /// Retire the peer's slot only when it still refers to `connection`,
    /// closing that connection so the peer learns its slot is gone. A stale
    /// failure from a superseded exchange must neither tear down the
    /// authoritative replacement nor leave the retired connection silently
    /// open on the peer.
    pub(crate) async fn retire_if_current_connection(
        &self,
        peer: &AgentId,
        connection: &quinn::Connection,
        reason: &'static [u8],
    ) {
        let retired = {
            let mut state = self.state.write().await;
            let is_current = state
                .slots
                .get(peer)
                .is_some_and(|slot| slot.connection.stable_id() == connection.stable_id());
            if is_current {
                let slot = state.slots.remove(peer).expect("checked above");
                *state.generations.entry(peer.clone()).or_default() += 1;
                Some(slot.connection)
            } else {
                None
            }
        };
        // Close outside the lock: the close frame flushes asynchronously.
        if let Some(retired) = retired {
            retired.close(0u32.into(), reason);
        }
    }

    pub(crate) async fn close_peer(&self, peer: &AgentId, reason: &'static [u8]) {
        let mut state = self.state.write().await;
        if let Some(slot) = state.slots.remove(peer) {
            slot.connection.close(0u32.into(), reason);
        }
        *state.generations.entry(peer.clone()).or_default() += 1;
    }

    pub(crate) async fn close_all(&self) {
        let mut state = self.state.write().await;
        for slot in state.slots.values() {
            slot.connection.close(0u32.into(), b"shutdown");
        }
        state.slots.clear();
    }

    pub(crate) async fn count(&self) -> usize {
        self.state.read().await.slots.len()
    }
}
