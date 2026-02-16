use std::sync::Arc;

use anyhow::Result;
use tracing::debug;

use super::{ClientState, IpcHandlers, SubscriptionFilter};
use crate::ipc::protocol::{DaemonReply, IpcErrorCode, WhoamiInfo, validate_raw_kinds};

use super::IPC_VERSION;

impl IpcHandlers {
    pub(super) async fn handle_whoami(&self, req_id: Option<String>) -> Result<DaemonReply> {
        Ok(DaemonReply::Whoami {
            ok: true,
            info: WhoamiInfo {
                agent_id: self.config.agent_id.clone(),
                public_key: self.config.public_key.clone(),
                name: self.config.name.clone(),
                version: self.config.version.clone(),
                ipc_version: IPC_VERSION,
                uptime_secs: (self.config.uptime_secs)(),
            },
            req_id,
        })
    }

    pub(super) async fn handle_inbox(
        &self,
        consumer: &str,
        limit: usize,
        raw_kinds: Option<Vec<String>>,
        req_id: Option<String>,
    ) -> Result<DaemonReply> {
        // Validate kinds
        let kinds = match raw_kinds {
            Some(ref raw) => match validate_raw_kinds(raw) {
                Ok(k) => Some(k),
                Err(_) => {
                    return Ok(DaemonReply::Error {
                        ok: false,
                        error: IpcErrorCode::InvalidCommand,
                        req_id,
                    });
                }
            },
            None => None,
        };

        let mut buf = self.receive_buffer.lock().await;
        let (messages, next_seq, has_more) = buf.fetch(consumer, limit, kinds.as_deref());
        if let Some(seq) = next_seq {
            buf.update_delivered_seq(consumer, seq);
        }
        drop(buf);

        Ok(DaemonReply::Inbox {
            ok: true,
            messages,
            next_seq,
            has_more,
            req_id,
        })
    }

    pub(super) async fn handle_ack(
        &self,
        consumer: &str,
        up_to_seq: u64,
        req_id: Option<String>,
    ) -> Result<DaemonReply> {
        match self.receive_buffer.lock().await.ack(consumer, up_to_seq) {
            Ok(acked_seq) => Ok(DaemonReply::Ack {
                ok: true,
                acked_seq,
                req_id,
            }),
            Err(_) => Ok(DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::AckOutOfRange,
                req_id,
            }),
        }
    }

    pub(super) async fn handle_subscribe(
        &self,
        client_id: u64,
        consumer: &str,
        replay: bool,
        raw_kinds: Option<Vec<String>>,
        req_id: Option<String>,
    ) -> Result<DaemonReply> {
        // Validate kinds
        let kinds = match raw_kinds {
            Some(ref raw) => match validate_raw_kinds(raw) {
                Ok(k) => Some(k),
                Err(_) => {
                    return Ok(DaemonReply::Error {
                        ok: false,
                        error: IpcErrorCode::InvalidCommand,
                        req_id,
                    });
                }
            },
            None => None,
        };

        let mut buf = self.receive_buffer.lock().await;
        let replay_to_seq = buf.highest_seq();

        // Replay buffered messages if requested
        let mut replayed = 0;
        if replay {
            let messages = buf.replay_messages(consumer, replay_to_seq, kinds.as_deref());
            drop(buf);

            // Snapshot the client sender to avoid holding the clients lock during replay
            let tx = self.clients.lock().await.get(&client_id).cloned();

            if let Some(tx) = tx {
                let mut highest_replayed_seq = None;
                for msg in &messages {
                    let event = DaemonReply::InboundEvent {
                        event: "inbound",
                        replay: true,
                        seq: msg.seq,
                        buffered_at_ms: msg.buffered_at_ms,
                        envelope: msg.envelope.clone(),
                    };
                    if let Ok(json) = serde_json::to_string(&event) {
                        if tx.try_send(Arc::from(json)).is_ok() {
                            replayed += 1;
                            highest_replayed_seq = Some(msg.seq);
                        } else {
                            break; // Queue full, stop replaying
                        }
                    }
                }
                // Update delivered seq only for what was actually sent
                if let Some(seq) = highest_replayed_seq {
                    self.receive_buffer
                        .lock()
                        .await
                        .update_delivered_seq(consumer, seq);
                }
            }
        } else {
            drop(buf);
        }

        // Set subscription filter
        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);
        let filter = SubscriptionFilter {
            kinds: kinds.clone(),
            replay_to_seq,
        };
        debug!(client_id, kinds = ?filter.kinds, "IPC client subscribed");
        state.subscription = Some(filter);

        Ok(DaemonReply::Subscribe {
            ok: true,
            subscribed: true,
            replayed,
            replay_to_seq: Some(replay_to_seq),
            req_id,
        })
    }
}
