use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

use crate::message::{AgentId, Envelope, MessageKind};

pub const MAX_PENDING_REQUESTS: usize = 1024;

#[derive(Debug)]
pub struct RequestDelivery {
    pub client_id: u64,
    pub request_id: Uuid,
    request: Arc<Envelope>,
    response: oneshot::Receiver<Envelope>,
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
}

#[derive(Debug)]
struct PendingRequest {
    owner: u64,
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

    pub async fn begin(&self, request: Arc<Envelope>) -> BeginRequest {
        let mut state = self.state.lock().await;
        let Some(client_id) = state.handler else {
            return BeginRequest::Respond(self.error_response(
                &request,
                "unhandled",
                "no application handler is registered",
                false,
            ));
        };
        if state.pending.contains_key(&request.id) {
            return BeginRequest::Respond(self.error_response(
                &request,
                "overloaded",
                "request identifier is already pending",
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
                self.state.lock().await.pending.remove(&delivery.request_id);
                self.error_response(
                    &delivery.request,
                    "timeout",
                    "request handler timed out",
                    true,
                )
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
        let Some(pending) = state.pending.remove(&request_id) else {
            return Err(BrokerError::RequestNotFound);
        };
        if pending.owner != client_id {
            state.pending.insert(request_id, pending);
            return Err(BrokerError::NotHandler);
        }
        let response =
            Envelope::response_to(&pending.request, self.local_agent_id.clone(), kind, payload);
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
