use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// When this slot became authoritative. Bounds the window during which a
    /// preferred-direction candidate may still replace it (see `admit_gated`).
    installed_at: Instant,
}

#[derive(Default)]
struct RegistryState {
    slots: HashMap<AgentId, ConnectionSlot>,
    generations: HashMap<AgentId, u64>,
}

impl RegistryState {
    /// Remove the peer's slot when its connection has closed, advancing the
    /// generation so stale attempt/teardown outcomes can neither mutate nor
    /// resurrect the emptied slot. The caller must hold the state write lock.
    fn reap_closed(&mut self, peer: &AgentId) {
        if self
            .slots
            .get(peer)
            .is_some_and(|slot| slot.connection.close_reason().is_some())
        {
            self.slots.remove(peer);
            *self.generations.entry(peer.clone()).or_default() += 1;
        }
    }
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

    /// The peer's authoritative connection, if any.
    ///
    /// Reads lazily reap a closed slot (removing it and advancing the
    /// generation), so a dead slot never lingers as authoritative and stale
    /// outcomes cannot mutate the emptied slot. That cleanup is a mutation,
    /// which is why this takes the write lock despite being a query.
    pub(crate) async fn live_slot(&self, peer: &AgentId) -> Option<quinn::Connection> {
        let mut state = self.state.write().await;
        state.reap_closed(peer);
        state.slots.get(peer).map(|slot| slot.connection.clone())
    }

    /// Admission with an authorization gate consulted atomically against
    /// slot installation.
    ///
    /// The gate is a SYNCHRONOUS closure (pin-snapshot enrollment check plus
    /// epoch comparison — both plain std-lock reads) that runs inside the
    /// registry's write-lock critical section, linearizing it against every
    /// mutation that closes slots through this registry (notably
    /// revocation's `close_peer`): either the gate observes the authority
    /// change and refuses, or the subsequent slot-closing mutation lands
    /// after installation and tears the fresh slot down. Because the gate
    /// cannot await, no lock is ever held across an await, no stall can
    /// block admission, and no lock-ordering rule between the registry and
    /// the directory is required.
    pub(crate) async fn admit_gated(
        &self,
        peer: AgentId,
        connection: quinn::Connection,
        direction: Direction,
        gate: impl FnOnce() -> bool,
    ) -> Admission {
        self.admit_gated_with_window(
            peer,
            connection,
            direction,
            gate,
            // A genuine simultaneous cross-dial handshake must complete within
            // one dial timeout of the incumbent's installation (see the
            // selection comment below).
            super::DIAL_TIMEOUT,
        )
        .await
    }

    /// Admission with the cross-dial replacement window made explicit so the
    /// aged-incumbent rule is unit-testable without real sleeps.
    pub(crate) async fn admit_gated_with_window(
        &self,
        peer: AgentId,
        connection: quinn::Connection,
        direction: Direction,
        gate: impl FnOnce() -> bool,
        cross_dial_window: Duration,
    ) -> Admission {
        let mut state = self.state.write().await;
        if !gate() {
            drop(state);
            connection.close(0u32.into(), b"peer not admitted");
            return Admission::Rejected;
        }
        state.reap_closed(&peer);
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
            // SPEC.md §Connection Lifecycle 4: a healthy incumbent wins within
            // its generation. Direction is ONLY a tie-breaker for simultaneous
            // cross-dials (Q-006/DEC-014): two handshakes that overlapped in
            // time. Any genuine racing handshake must have started before the
            // incumbent was installed and completes within DIAL_TIMEOUT of its
            // own start, so every member of the race arrives within
            // DIAL_TIMEOUT of installation. A later preferred-direction
            // candidate is not part of that race: it loses to the healthy
            // incumbent and is closed as an ordinary duplicate instead of
            // evicting a proven connection solely for its direction.
            let within_cross_dial_window = incumbent.installed_at.elapsed() <= cross_dial_window;
            if incumbent.direction == preferred
                || direction != preferred
                || !within_cross_dial_window
            {
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
                installed_at: Instant::now(),
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

#[cfg(test)]
#[path = "connection_registry_tests.rs"]
mod tests;
