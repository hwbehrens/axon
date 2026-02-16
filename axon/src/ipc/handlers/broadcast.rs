use std::sync::Arc;

use anyhow::Result;

use super::{IpcHandlers, SubscriptionFilter};
use crate::ipc::protocol::DaemonReply;
use crate::message::Envelope;

/// Snapshot of a client's routing info for broadcast, avoiding holding locks during sends.
struct BroadcastTarget {
    tx: tokio::sync::mpsc::Sender<Arc<str>>,
    version: Option<u32>,
    filter: Option<SubscriptionFilter>,
    consumer: String,
}

impl IpcHandlers {
    pub async fn broadcast_inbound(&self, envelope: &Envelope) -> Result<()> {
        // Step 1: Push to buffer (lock briefly, then release)
        let (seq, buffered_at_ms) = self.receive_buffer.lock().await.push(envelope.clone());

        // Step 2: Snapshot client info (lock clients + states together, then release both)
        let targets: Vec<BroadcastTarget> = {
            let clients = self.clients.lock().await;
            let states = self.client_states.lock().await;
            clients
                .iter()
                .filter_map(|(client_id, tx)| {
                    let state = states.get(client_id)?;
                    Some(BroadcastTarget {
                        tx: tx.clone(),
                        version: state.version,
                        filter: state.subscription.clone(),
                        consumer: state.consumer.clone(),
                    })
                })
                .collect()
        };
        // Both locks are now released

        // Step 3: Serialize once per message type (optimization: avoid per-client serialization)
        let v1_line: Option<Arc<str>> = {
            let msg = DaemonReply::Inbound {
                inbound: true,
                envelope: envelope.clone(),
            };
            serde_json::to_string(&msg).ok().map(Arc::from)
        };

        let v2_line: Option<Arc<str>> = {
            let event = DaemonReply::InboundEvent {
                event: "inbound",
                replay: false,
                seq,
                buffered_at_ms,
                envelope: envelope.clone(),
            };
            serde_json::to_string(&event).ok().map(Arc::from)
        };

        // Step 4: Send to each client without holding any locks
        let mut delivered_consumers: Vec<String> = Vec::new();

        for target in &targets {
            if target.version.unwrap_or(1) < 2 {
                // v1 client: legacy broadcast
                if let Some(line) = &v1_line {
                    let _ = target.tx.try_send(line.clone());
                }
                continue;
            }

            // v2+ client: only send if subscribed and filter matches
            if let Some(filter) = &target.filter
                && filter.matches(&envelope.kind)
                && seq > filter.replay_to_seq
                && let Some(line) = &v2_line
                && target.tx.try_send(line.clone()).is_ok()
            {
                delivered_consumers.push(target.consumer.clone());
            }
        }

        // Step 5: Update delivered seq (lock buffer briefly)
        if !delivered_consumers.is_empty() {
            let mut buf = self.receive_buffer.lock().await;
            for consumer in delivered_consumers {
                buf.update_delivered_seq(&consumer, seq);
            }
        }

        Ok(())
    }
}
