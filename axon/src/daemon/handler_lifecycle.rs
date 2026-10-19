//! Handler-lease lifecycle helpers shared by the daemon event loop and the
//! transport response handler.
//!
//! Delivery failures are classified so an oversized (but wire-legal) request
//! event fails only that request while a gone or queue-overflowed handler
//! loses its lease (spec/IPC.md §2, §6.2, §7).

use std::sync::Arc;

use tracing::warn;

use crate::ipc::{IpcServer, RequestEventError};
use crate::request_broker::{BeginRequest, RequestBroker};
use crate::transport::ResponseHandlerFn;

use super::INBOUND_REQUEST_TIMEOUT;

pub(super) fn make_response_handler(ipc: IpcServer, broker: RequestBroker) -> ResponseHandlerFn {
    Arc::new(move |request| {
        let ipc = ipc.clone();
        let broker = broker.clone();
        Box::pin(async move {
            match broker.begin(request.clone(), INBOUND_REQUEST_TIMEOUT).await {
                BeginRequest::Respond(response) => Some(response),
                BeginRequest::Deliver(delivery) => {
                    match ipc
                        .try_send_request_event(delivery.client_id, &request)
                        .await
                    {
                        Ok(()) => {}
                        Err(RequestEventError::Disconnected | RequestEventError::QueueOverflow) => {
                            // Release the gone/overflow-closed handler's lease (B1).
                            broker.disconnect(delivery.client_id).await;
                        }
                        Err(err) => {
                            // An oversized or unserializable event is not a
                            // gone handler: the client is healthy and keeps its
                            // lease. Fail only the affected request with one
                            // terminal, retryable error (spec/IPC.md §2, §6.2).
                            warn!(
                                client_id = delivery.client_id,
                                error = %err,
                                "failed delivering request event; failing only the request"
                            );
                            // `from` is TLS-authenticated before broker entry,
                            // so it scopes the terminal failure to this peer.
                            if let Some(peer) = request.from.as_ref() {
                                broker
                                    .fail(
                                        peer,
                                        delivery.request_id,
                                        "overloaded",
                                        "request handler could not receive the request",
                                        true,
                                    )
                                    .await;
                            }
                        }
                    }
                    Some(
                        broker
                            .await_response(delivery, INBOUND_REQUEST_TIMEOUT)
                            .await,
                    )
                }
            }
        })
    })
}

/// Release the handler lease if its client is already gone (B1).
pub(super) async fn release_lease_if_client_gone(
    ipc: &IpcServer,
    broker: &RequestBroker,
    client_id: u64,
) {
    if !ipc.connected_client_ids().await.contains(&client_id) {
        broker.disconnect(client_id).await;
    }
}

/// Reconcile broker state against the live IPC clients after a lagged
/// broadcast delivery: lost disconnect notifications must not leave a gone
/// handler pinning its lease or its pending requests (I16).
pub(super) async fn reconcile_after_lag(ipc: &IpcServer, broker: &RequestBroker) {
    let live: std::collections::HashSet<_> = ipc.connected_client_ids().await.into_iter().collect();
    broker.reconcile_clients(&live).await;
}
