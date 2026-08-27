use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

use crate::message::{AgentId, Envelope, MAX_MESSAGE_SIZE, MessageKind};

pub const MAX_PENDING_REQUESTS: usize = 1024;
/// Completed-request responses are cached so transport-level retries can be
/// answered without re-executing application side effects.
const MAX_COMPLETED_CACHE_ENTRIES: usize = 256;
const MAX_COMPLETED_CACHE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct RequestDelivery {
    pub client_id: u64,
    pub request_id: Uuid,
    request: Arc<Envelope>,
    /// Peer-scoped correlation key matching the pending entry for this
    /// delivery; used to remove exactly this request on timeout.
    key: RequestKey,
    response: oneshot::Receiver<Envelope>,
}

/// Correlation key scoped to the authenticated remote peer.
///
/// A request UUID is chosen by the remote side, so two peers can present
/// the same UUID. Pending entries and cached terminal responses are keyed
/// by `(peer, uuid)` so one peer's cached outcome can never be replayed to
/// — or satisfied by a stale reply for — another peer's exchange.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    peer: AgentId,
    id: Uuid,
}

impl RequestKey {
    /// Derive the key from an inbound envelope. `from` is overwritten with
    /// TLS-derived identity before broker entry, so it is authoritative.
    fn of(envelope: &Envelope) -> Option<Self> {
        envelope.from.as_ref().map(|peer| Self {
            peer: peer.clone(),
            id: envelope.id,
        })
    }
}

#[derive(Debug)]
struct CompletedResponse {
    key: RequestKey,
    response: Envelope,
    wire_bytes: usize,
}

#[derive(Debug, Default)]
struct CompletedCache {
    queue: VecDeque<CompletedResponse>,
    total_bytes: usize,
}

impl CompletedCache {
    fn remember(&mut self, key: RequestKey, response: Envelope) {
        let wire_bytes = serde_json::to_vec(&response)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        // Oversized responses would dominate the budget; skip caching them
        // rather than evicting everything else.
        if wire_bytes > MAX_COMPLETED_CACHE_BYTES {
            return;
        }
        while self.total_bytes + wire_bytes > MAX_COMPLETED_CACHE_BYTES
            || self.queue.len() >= MAX_COMPLETED_CACHE_ENTRIES
        {
            let Some(evicted) = self.queue.pop_front() else {
                break;
            };
            self.total_bytes -= evicted.wire_bytes;
        }
        self.total_bytes += wire_bytes;
        self.queue.push_back(CompletedResponse {
            key,
            response,
            wire_bytes,
        });
    }

    fn replay(&self, key: &RequestKey) -> Option<Envelope> {
        self.queue
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.response.clone())
    }
}

#[derive(Debug)]
pub enum BeginRequest {
    Deliver(RequestDelivery),
    Respond(Envelope),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerError {
    HandlerBusy,
    NotHandler,
    RequestNotFound,
    /// The request ID matches more than one peer-scoped pending request and
    /// no `peer` was supplied to disambiguate.
    AmbiguousRequest,
    InvalidPayload,
}

#[derive(Debug, Clone)]
pub struct RequestBroker {
    local_agent_id: AgentId,
    state: Arc<Mutex<BrokerState>>,
}

#[derive(Debug, Default)]
struct BrokerState {
    handler: Option<u64>,
    pending: HashMap<RequestKey, PendingRequest>,
    completed: CompletedCache,
}

#[derive(Debug)]
struct PendingRequest {
    owner: u64,
    inserted_at: Instant,
    request: Arc<Envelope>,
    response: oneshot::Sender<Envelope>,
}

impl RequestBroker {
    pub fn new(local_agent_id: AgentId) -> Self {
        Self {
            local_agent_id,
            state: Arc::new(Mutex::new(BrokerState::default())),
        }
    }

    pub async fn register(&self, client_id: u64) -> Result<(), BrokerError> {
        let mut state = self.state.lock().await;
        match state.handler {
            None => {
                state.handler = Some(client_id);
                Ok(())
            }
            Some(owner) if owner == client_id => Ok(()),
            Some(_) => Err(BrokerError::HandlerBusy),
        }
    }

    pub async fn begin(&self, request: Arc<Envelope>, pending_ttl: Duration) -> BeginRequest {
        let mut state = self.state.lock().await;
        let Some(key) = RequestKey::of(&request) else {
            return BeginRequest::Respond(self.error_response(
                &request,
                "invalid_envelope",
                "request lacks an authenticated origin identity",
                false,
            ));
        };
        // A cached terminal response answers any redelivery of the same
        // (peer, request UUID) pair. This runs BEFORE the handler lookup so
        // that (a) a retried exchange after handler loss replays the recorded
        // outcome instead of reporting `unhandled`, and (b) a swept-orphaned
        // request is never re-delivered — its key is tombstoned, so a stale
        // handler's late `reply` can only ever hit RequestNotFound and can
        // never satisfy a newer delivery.
        if let Some(replay) = state.completed.replay(&key) {
            return BeginRequest::Respond(replay);
        }
        let Some(client_id) = state.handler else {
            return BeginRequest::Respond(self.error_response(
                &request,
                "unhandled",
                format!(
                    "no application handler registered for request '{}'",
                    request.id
                ),
                false,
            ));
        };
        // Awaited tasks can be cancelled mid-flight (QUIC connection loss),
        // orphaning their entries: nothing is left to observe the deadline.
        // Sweep expired orphans lazily so cancellation cannot exhaust the
        // map; their terminal timeout is tombstoned so a peer-level retry of
        // the same envelope receives it rather than a fresh delivery.
        let expired: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, pending)| pending.inserted_at.elapsed() > pending_ttl)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(pending) = state.pending.remove(&key) {
                let response = self.error_response(
                    &pending.request,
                    "timeout",
                    "request timed out waiting for a response",
                    true,
                );
                state.completed.remember(key, response.clone());
                let _ = pending.response.send(response);
            }
        }
        // The sweep above may have just tombstoned an earlier attempt of
        // THIS request key. Without this recheck a same-call retry would
        // fall through to a fresh delivery that the stale attempt's late
        // `reply` could satisfy — the exact conflation tombstones exist to
        // prevent.
        if let Some(replay) = state.completed.replay(&key) {
            return BeginRequest::Respond(replay);
        }
        if state.pending.contains_key(&key) {
            return BeginRequest::Respond(self.error_response(
                &request,
                "overloaded",
                "duplicate of an in-flight request; retry after it resolves",
                true,
            ));
        }

        if state.pending.len() >= MAX_PENDING_REQUESTS {
            return BeginRequest::Respond(self.error_response(
                &request,
                "overloaded",
                "inbound request capacity is exhausted",
                true,
            ));
        }
        let (response, receiver) = oneshot::channel();
        state.pending.insert(
            key.clone(),
            PendingRequest {
                owner: client_id,
                inserted_at: Instant::now(),
                request: request.clone(),
                response,
            },
        );
        BeginRequest::Deliver(RequestDelivery {
            client_id,
            request_id: request.id,
            key,
            request,
            response: receiver,
        })
    }

    pub async fn await_response(
        &self,
        delivery: RequestDelivery,
        deadline: std::time::Duration,
    ) -> Envelope {
        match tokio::time::timeout(deadline, delivery.response).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => self.error_response(
                &delivery.request,
                "unhandled",
                "request handler disconnected",
                true,
            ),
            Err(_) => {
                let timeout_response = self.error_response(
                    &delivery.request,
                    "timeout",
                    "request handler timed out",
                    true,
                );
                let mut state = self.state.lock().await;
                if state.pending.remove(&delivery.key).is_some() {
                    state
                        .completed
                        .remember(delivery.key.clone(), timeout_response.clone());
                }
                timeout_response
            }
        }
    }

    /// Resolve a pending inbound request on behalf of the handler.
    ///
    /// Requests are correlated per authenticated remote peer, so
    /// `request_id` alone can be ambiguous when two peers present the same
    /// UUID. Supply `peer` (the `from` identity delivered with the request)
    /// whenever the caller knows it; without it, an ID matching multiple
    /// pending requests fails with [`BrokerError::AmbiguousRequest`] rather
    /// than replying to an arbitrarily chosen peer.
    pub async fn reply(
        &self,
        client_id: u64,
        request_id: Uuid,
        kind: MessageKind,
        payload: Value,
        peer: Option<AgentId>,
    ) -> Result<(), BrokerError> {
        let mut state = self.state.lock().await;
        if state.handler != Some(client_id) {
            return Err(BrokerError::NotHandler);
        }
        if !payload.is_object() {
            return Err(BrokerError::InvalidPayload);
        }
        // Resolve candidates scoped by owning handler and (when supplied) the
        // requesting peer; UUID collisions across peers must never conflate
        // exchanges.
        let mut candidates: Vec<RequestKey> = state
            .pending
            .iter()
            .filter(|(key, pending)| {
                key.id == request_id
                    && pending.owner == client_id
                    && peer.as_ref().is_none_or(|peer| &key.peer == peer)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let key = match candidates.len() {
            0 => return Err(BrokerError::RequestNotFound),
            1 => candidates.pop().expect("exactly one candidate"),
            _ => return Err(BrokerError::AmbiguousRequest),
        };
        let Some(pending) = state.pending.remove(&key) else {
            return Err(BrokerError::RequestNotFound);
        };
        let response =
            Envelope::response_to(&pending.request, self.local_agent_id.clone(), kind, payload);
        // Reject BEFORE consuming the pending entry: transport framing would
        // silently drop an over-limit response while IPC already reported
        // success. A rejected reply leaves the request pending so the
        // handler can send a smaller one.
        if response
            .wire_encode()
            .map(|bytes| bytes.len() > MAX_MESSAGE_SIZE as usize)
            .unwrap_or(true)
        {
            // Leave the request pending so the handler can send a smaller
            // reply.
            state.pending.insert(key, pending);
            return Err(BrokerError::InvalidPayload);
        }
        state.completed.remember(key, response.clone());
        pending
            .response
            .send(response)
            .map_err(|_| BrokerError::RequestNotFound)
    }

    pub async fn fail(
        &self,
        peer: &AgentId,
        request_id: Uuid,
        code: &'static str,
        message: &'static str,
        retryable: bool,
    ) {
        let key = RequestKey {
            peer: peer.clone(),
            id: request_id,
        };
        let mut state = self.state.lock().await;
        if let Some(pending) = state.pending.remove(&key) {
            let response = self.error_response(&pending.request, code, message, retryable);
            state.completed.remember(key, response.clone());
            drop(state);
            let _ = pending.response.send(response);
        }
    }

    pub async fn disconnect(&self, client_id: u64) {
        let mut state = self.state.lock().await;
        if state.handler != Some(client_id) {
            return;
        }
        state.handler = None;
        let pending_keys: Vec<_> = state
            .pending
            .iter()
            .filter_map(|(key, pending)| (pending.owner == client_id).then_some(key.clone()))
            .collect();
        for key in pending_keys {
            if let Some(pending) = state.pending.remove(&key) {
                let response = self.error_response(
                    &pending.request,
                    "unhandled",
                    "application handler disconnected before replying",
                    true,
                );
                state.completed.remember(key, response.clone());
                let _ = pending.response.send(response);
            }
        }
    }

    /// Drop the handler lease and pending requests for clients that are no
    /// longer connected. Used to reconcile broker state when IPC disconnect
    /// notifications are lost (broadcast lag).
    pub async fn reconcile_clients(&self, live_clients: &HashSet<u64>) {
        let mut state = self.state.lock().await;
        if let Some(holder) = state.handler
            && !live_clients.contains(&holder)
        {
            state.handler = None;
        }
        let orphans: Vec<_> = state
            .pending
            .iter()
            .filter_map(|(key, pending)| {
                (!live_clients.contains(&pending.owner)).then_some(key.clone())
            })
            .collect();
        for key in orphans {
            if let Some(pending) = state.pending.remove(&key) {
                let response = self.error_response(
                    &pending.request,
                    "unhandled",
                    "application handler disconnected before replying",
                    true,
                );
                state.completed.remember(key, response.clone());
                let _ = pending.response.send(response);
            }
        }
    }

    pub async fn pending_count(&self) -> usize {
        self.state.lock().await.pending.len()
    }

    fn error_response(
        &self,
        request: &Envelope,
        code: &'static str,
        message: impl Into<String>,
        retryable: bool,
    ) -> Envelope {
        let message = message.into();
        Envelope::response_to(
            request,
            self.local_agent_id.clone(),
            MessageKind::Error,
            json!({
                "code": code,
                "message": message,
                "retryable": retryable,
            }),
        )
    }
}

#[cfg(test)]
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "replay_tests.rs"]
mod replay_tests;
