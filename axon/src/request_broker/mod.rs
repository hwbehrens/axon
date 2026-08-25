use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

use crate::message::{AgentId, Envelope, MessageKind};

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
    response: oneshot::Receiver<Envelope>,
}

#[derive(Debug)]
struct CompletedResponse {
    request_id: Uuid,
    response: Envelope,
    wire_bytes: usize,
}

#[derive(Debug, Default)]
struct CompletedCache {
    queue: VecDeque<CompletedResponse>,
    total_bytes: usize,
}

impl CompletedCache {
    fn remember(&mut self, request_id: Uuid, response: Envelope) {
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
            request_id,
            response,
            wire_bytes,
        });
    }

    fn replay(&self, request_id: &Uuid) -> Option<Envelope> {
        self.queue
            .iter()
            .find(|entry| entry.request_id == *request_id)
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
    pending: HashMap<Uuid, PendingRequest>,
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
        let Some(client_id) = state.handler else {
            return BeginRequest::Respond(self.error_response(
                &request,
                "unhandled",
                "no application handler is registered",
                false,
            ));
        };
        // A cached completion answers a retried exchange without re-running
        // the application handler.
        if let Some(replay) = state.completed.replay(&request.id) {
            return BeginRequest::Respond(replay);
        }
        // Awaited tasks can be cancelled mid-flight (QUIC connection loss),
        // orphaning their entries: nothing is left to observe the deadline.
        // Sweep expired orphans lazily so cancellation cannot exhaust the map.
        let expired: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, pending)| pending.inserted_at.elapsed() > pending_ttl)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(pending) = state.pending.remove(&id) {
                let _ = pending.response.send(self.error_response(
                    &pending.request,
                    "timeout",
                    "request timed out waiting for a response",
                    true,
                ));
            }
        }
        if state.pending.contains_key(&request.id) {
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
            request.id,
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
                if state.pending.remove(&delivery.request_id).is_some() {
                    state
                        .completed
                        .remember(delivery.request_id, timeout_response.clone());
                }
                timeout_response
            }
        }
    }

    pub async fn reply(
        &self,
        client_id: u64,
        request_id: Uuid,
        kind: MessageKind,
        payload: Value,
    ) -> Result<(), BrokerError> {
        let mut state = self.state.lock().await;
        if state.handler != Some(client_id) {
            return Err(BrokerError::NotHandler);
        }
        if !payload.is_object() {
            return Err(BrokerError::InvalidPayload);
        }
        let Some(pending) = state.pending.remove(&request_id) else {
            return Err(BrokerError::RequestNotFound);
        };
        if pending.owner != client_id {
            state.pending.insert(request_id, pending);
            return Err(BrokerError::NotHandler);
        }
        let response =
            Envelope::response_to(&pending.request, self.local_agent_id.clone(), kind, payload);
        state.completed.remember(request_id, response.clone());
        pending
            .response
            .send(response)
            .map_err(|_| BrokerError::RequestNotFound)
    }

    pub async fn fail(
        &self,
        request_id: Uuid,
        code: &'static str,
        message: &'static str,
        retryable: bool,
    ) {
        let pending = self.state.lock().await.pending.remove(&request_id);
        if let Some(pending) = pending {
            let response = self.error_response(&pending.request, code, message, retryable);
            let _ = pending.response.send(response);
        }
    }

    pub async fn disconnect(&self, client_id: u64) {
        let mut state = self.state.lock().await;
        if state.handler != Some(client_id) {
            return;
        }
        state.handler = None;
        let pending_ids: Vec<_> = state
            .pending
            .iter()
            .filter_map(|(id, pending)| (pending.owner == client_id).then_some(*id))
            .collect();
        for id in pending_ids {
            if let Some(pending) = state.pending.remove(&id) {
                let response = self.error_response(
                    &pending.request,
                    "unhandled",
                    "application handler disconnected before replying",
                    true,
                );
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
            .filter_map(|(id, pending)| (!live_clients.contains(&pending.owner)).then_some(*id))
            .collect();
        for id in orphans {
            if let Some(pending) = state.pending.remove(&id) {
                let _ = pending.response.send(self.error_response(
                    &pending.request,
                    "unhandled",
                    "application handler disconnected before replying",
                    true,
                ));
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
        message: &'static str,
        retryable: bool,
    ) -> Envelope {
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
#[path = "tests.rs"]
mod tests;
