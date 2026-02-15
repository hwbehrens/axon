use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::backend::IpcBackend;
use super::protocol::{DaemonReply, IpcCommand, IpcErrorCode, WhoamiInfo, validate_raw_kinds};
use super::receive_buffer::ReceiveBuffer;
use crate::message::{Envelope, MessageKind};

use super::backend::{SendResult, StatusResult};

const IPC_VERSION: u32 = 2;
const MAX_CONSUMER_LEN: usize = 64;

/// Result of dispatching an IPC command through the unified handler.
pub struct DispatchResult {
    pub reply: DaemonReply,
    pub response_envelope: Option<Envelope>,
    /// If true, the daemon should close this client after sending the reply.
    pub close: bool,
}

/// No-op backend used by `handle_command` (for tests and v2-only commands
/// that don't need daemon resources). Send/Peers/Status will return
/// InternalError if called without a real backend.
struct NoopBackend;

impl IpcBackend for NoopBackend {
    async fn send_message(
        &self,
        _to: String,
        _kind: MessageKind,
        _payload: serde_json::Value,
        _ref_id: Option<uuid::Uuid>,
    ) -> Result<SendResult> {
        anyhow::bail!("no backend available")
    }
    async fn peers(&self) -> Result<Vec<super::protocol::PeerSummary>> {
        anyhow::bail!("no backend available")
    }
    async fn status(&self) -> Result<StatusResult> {
        anyhow::bail!("no backend available")
    }
}

// ---------------------------------------------------------------------------
// Client state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SubscriptionFilter {
    pub kinds: Option<Vec<MessageKind>>,
    pub replay_to_seq: u64,
}

impl SubscriptionFilter {
    pub fn matches(&self, kind: &MessageKind) -> bool {
        match &self.kinds {
            None => true,
            Some(kinds) => kinds.contains(kind),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClientState {
    pub version: Option<u32>,
    pub authenticated: bool,
    pub subscription: Option<SubscriptionFilter>,
    pub consumer: String,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub struct IpcServerConfig {
    pub agent_id: String,
    pub public_key: String,
    pub name: Option<String>,
    pub version: String,
    pub token: Option<String>,
    pub buffer_size: usize,
    pub buffer_ttl_secs: u64,
    pub buffer_byte_cap: Option<usize>,
    pub allow_v1: bool,
    pub uptime_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Default for IpcServerConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            public_key: String::new(),
            name: None,
            version: "0.1.0".to_string(),
            token: None,
            buffer_size: 1000,
            buffer_ttl_secs: 86400,
            buffer_byte_cap: None,
            allow_v1: true,
            uptime_secs: Arc::new(|| 0),
        }
    }
}

// ---------------------------------------------------------------------------
// IPC command handlers
// ---------------------------------------------------------------------------

pub struct IpcHandlers {
    config: Arc<IpcServerConfig>,
    client_states: Arc<Mutex<HashMap<u64, ClientState>>>,
    receive_buffer: Arc<Mutex<ReceiveBuffer>>,
    clients: Arc<Mutex<HashMap<u64, tokio::sync::mpsc::Sender<Arc<str>>>>>,
}

impl IpcHandlers {
    pub fn new(
        config: Arc<IpcServerConfig>,
        client_states: Arc<Mutex<HashMap<u64, ClientState>>>,
        receive_buffer: Arc<Mutex<ReceiveBuffer>>,
        clients: Arc<Mutex<HashMap<u64, tokio::sync::mpsc::Sender<Arc<str>>>>>,
    ) -> Self {
        Self {
            config,
            client_states,
            receive_buffer,
            clients,
        }
    }

    pub async fn handle_command(&self, client_id: u64, command: IpcCommand) -> Result<DaemonReply> {
        let result = self
            .dispatch_command_inner(client_id, command, None::<&NoopBackend>)
            .await?;
        Ok(result.reply)
    }

    /// Dispatch an IPC command with full policy enforcement (auth, hello,
    /// req_id gating) for ALL commands including Send/Peers/Status.
    pub async fn dispatch_command(
        &self,
        client_id: u64,
        command: IpcCommand,
        backend: &(impl IpcBackend + ?Sized),
    ) -> Result<DispatchResult> {
        self.dispatch_command_inner(client_id, command, Some(backend))
            .await
    }

    async fn dispatch_command_inner<B: IpcBackend + ?Sized>(
        &self,
        client_id: u64,
        command: IpcCommand,
        backend: Option<&B>,
    ) -> Result<DispatchResult> {
        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);

        let req_id = command.req_id().map(|s| s.to_string());

        // Hardened mode: reject any command before hello when allow_v1 = false
        if !self.config.allow_v1
            && state.version.is_none()
            && !matches!(command, IpcCommand::Hello { .. })
        {
            return Ok(DispatchResult {
                reply: DaemonReply::Error {
                    ok: false,
                    error: IpcErrorCode::HelloRequired,
                    req_id,
                },
                response_envelope: None,
                close: true,
            });
        }

        // v2-only commands require hello handshake first
        let is_v2_only_command = matches!(
            command,
            IpcCommand::Whoami { .. }
                | IpcCommand::Inbox { .. }
                | IpcCommand::Ack { .. }
                | IpcCommand::Subscribe { .. }
        );
        if is_v2_only_command && state.version.is_none() {
            return Ok(DispatchResult {
                reply: DaemonReply::Error {
                    ok: false,
                    error: IpcErrorCode::HelloRequired,
                    req_id,
                },
                response_envelope: None,
                close: false,
            });
        }

        // Check if this is a v2 client (has done hello)
        let is_v2_client = state.version.is_some();

        // v2 clients: ALL commands must include req_id (IPC.md §1.3)
        if is_v2_client && req_id.is_none() {
            return Ok(DispatchResult {
                reply: DaemonReply::Error {
                    ok: false,
                    error: IpcErrorCode::InvalidCommand,
                    req_id: None,
                },
                response_envelope: None,
                close: false,
            });
        }

        // Only require auth if the client has done hello (v2+)
        // v1 clients (version=None) bypass all auth checks
        if is_v2_client && !state.authenticated {
            let requires_auth = !matches!(
                command,
                IpcCommand::Hello { .. } | IpcCommand::Auth { .. } | IpcCommand::Status { .. }
            );

            if requires_auth {
                return Ok(DispatchResult {
                    reply: DaemonReply::Error {
                        ok: false,
                        error: IpcErrorCode::AuthRequired,
                        req_id,
                    },
                    response_envelope: None,
                    close: false,
                });
            }
        }

        let consumer = state.consumer.clone();
        drop(states);

        match command {
            IpcCommand::Hello {
                version,
                consumer: consumer_name,
                req_id,
                ..
            } => {
                let reply = self
                    .handle_hello(client_id, version, consumer_name, req_id)
                    .await?;
                let close = matches!(
                    &reply,
                    DaemonReply::Error {
                        error: IpcErrorCode::UnsupportedVersion,
                        ..
                    }
                );
                Ok(DispatchResult {
                    reply,
                    response_envelope: None,
                    close,
                })
            }
            IpcCommand::Auth { token, req_id, .. } => {
                let reply = self.handle_auth(client_id, token, req_id).await?;
                Ok(DispatchResult {
                    reply,
                    response_envelope: None,
                    close: false,
                })
            }
            IpcCommand::Whoami { req_id, .. } => {
                let reply = self.handle_whoami(req_id).await?;
                Ok(DispatchResult {
                    reply,
                    response_envelope: None,
                    close: false,
                })
            }
            IpcCommand::Inbox {
                limit,
                kinds,
                req_id,
                ..
            } => {
                let reply = self.handle_inbox(&consumer, limit, kinds, req_id).await?;
                Ok(DispatchResult {
                    reply,
                    response_envelope: None,
                    close: false,
                })
            }
            IpcCommand::Ack {
                up_to_seq, req_id, ..
            } => {
                let reply = self.handle_ack(&consumer, up_to_seq, req_id).await?;
                Ok(DispatchResult {
                    reply,
                    response_envelope: None,
                    close: false,
                })
            }
            IpcCommand::Subscribe {
                replay,
                kinds,
                req_id,
                ..
            } => {
                let reply = self
                    .handle_subscribe(client_id, &consumer, replay, kinds, req_id)
                    .await?;
                Ok(DispatchResult {
                    reply,
                    response_envelope: None,
                    close: false,
                })
            }
            IpcCommand::Send {
                to,
                kind,
                payload,
                ref_id,
                req_id,
            } => {
                let Some(backend) = backend else {
                    return Ok(DispatchResult {
                        reply: DaemonReply::Error {
                            ok: false,
                            error: IpcErrorCode::InternalError,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    });
                };
                match backend.send_message(to, kind, payload, ref_id).await {
                    Ok(result) => {
                        let response_envelope = result.response;
                        Ok(DispatchResult {
                            reply: DaemonReply::SendAck {
                                ok: true,
                                msg_id: result.msg_id,
                                req_id,
                            },
                            response_envelope,
                            close: false,
                        })
                    }
                    Err(e) => {
                        let error_code = match e.to_string().as_str() {
                            "peer_not_found" => IpcErrorCode::PeerNotFound,
                            "peer_unreachable" => IpcErrorCode::PeerUnreachable,
                            s if s.starts_with("invalid_command") => IpcErrorCode::InvalidCommand,
                            _ => IpcErrorCode::InternalError,
                        };
                        Ok(DispatchResult {
                            reply: DaemonReply::Error {
                                ok: false,
                                error: error_code,
                                req_id,
                            },
                            response_envelope: None,
                            close: false,
                        })
                    }
                }
            }
            IpcCommand::Peers { req_id } => {
                let Some(backend) = backend else {
                    return Ok(DispatchResult {
                        reply: DaemonReply::Error {
                            ok: false,
                            error: IpcErrorCode::InternalError,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    });
                };
                match backend.peers().await {
                    Ok(peers) => Ok(DispatchResult {
                        reply: DaemonReply::Peers {
                            ok: true,
                            peers,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    }),
                    Err(_) => Ok(DispatchResult {
                        reply: DaemonReply::Error {
                            ok: false,
                            error: IpcErrorCode::InternalError,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    }),
                }
            }
            IpcCommand::Status { req_id } => {
                let Some(backend) = backend else {
                    return Ok(DispatchResult {
                        reply: DaemonReply::Error {
                            ok: false,
                            error: IpcErrorCode::InternalError,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    });
                };
                match backend.status().await {
                    Ok(status) => Ok(DispatchResult {
                        reply: DaemonReply::Status {
                            ok: true,
                            uptime_secs: status.uptime_secs,
                            peers_connected: status.peers_connected,
                            messages_sent: status.messages_sent,
                            messages_received: status.messages_received,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    }),
                    Err(_) => Ok(DispatchResult {
                        reply: DaemonReply::Error {
                            ok: false,
                            error: IpcErrorCode::InternalError,
                            req_id,
                        },
                        response_envelope: None,
                        close: false,
                    }),
                }
            }
        }
    }

    async fn handle_hello(
        &self,
        client_id: u64,
        version: u32,
        consumer: String,
        req_id: Option<String>,
    ) -> Result<DaemonReply> {
        // Validate consumer name length
        if consumer.len() > MAX_CONSUMER_LEN {
            return Ok(DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::InvalidCommand,
                req_id,
            });
        }

        let negotiated = version.min(IPC_VERSION);

        // Hardened mode: reject v1 negotiation
        if !self.config.allow_v1 && negotiated < 2 {
            return Ok(DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::UnsupportedVersion,
                req_id,
            });
        }

        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);
        state.version = Some(negotiated);
        state.consumer = consumer;
        drop(states);

        debug!(client_id, negotiated, "IPC hello handshake");

        Ok(DaemonReply::Hello {
            ok: true,
            version: negotiated,
            daemon_max_version: IPC_VERSION,
            agent_id: self.config.agent_id.clone(),
            features: vec![
                "auth".to_string(),
                "buffer".to_string(),
                "subscribe".to_string(),
            ],
            req_id,
        })
    }

    async fn handle_auth(
        &self,
        client_id: u64,
        token: String,
        req_id: Option<String>,
    ) -> Result<DaemonReply> {
        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);

        // If already authenticated via peer creds, accept anyway
        if state.authenticated {
            info!(
                client_id,
                method = "peer_credentials",
                "IPC client authenticated"
            );
            return Ok(DaemonReply::Auth {
                ok: true,
                auth: "accepted".to_string(),
                req_id,
            });
        }

        // Check token if configured
        if let Some(expected_token) = &self.config.token {
            // Validate token format: must be exactly 64 hex characters
            if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
                warn!(client_id, "IPC auth failed: malformed token");
                return Ok(DaemonReply::Error {
                    ok: false,
                    error: IpcErrorCode::AuthFailed,
                    req_id,
                });
            }

            // Use constant-time comparison to prevent timing side-channel attacks
            use subtle::ConstantTimeEq;
            let token_bytes = token.as_bytes();
            let expected_bytes = expected_token.as_bytes();

            let tokens_match = token_bytes.ct_eq(expected_bytes).into();

            if tokens_match {
                state.authenticated = true;
                info!(client_id, method = "token", "IPC client authenticated");
                Ok(DaemonReply::Auth {
                    ok: true,
                    auth: "accepted".to_string(),
                    req_id,
                })
            } else {
                debug!(client_id, "IPC auth failed: invalid token");
                Ok(DaemonReply::Error {
                    ok: false,
                    error: IpcErrorCode::AuthFailed,
                    req_id,
                })
            }
        } else {
            debug!(
                client_id,
                "IPC auth rejected: no token configured on server"
            );
            Ok(DaemonReply::Error {
                ok: false,
                error: IpcErrorCode::AuthFailed,
                req_id,
            })
        }
    }

    async fn handle_whoami(&self, req_id: Option<String>) -> Result<DaemonReply> {
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

    async fn handle_inbox(
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

    async fn handle_ack(
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

    async fn handle_subscribe(
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

            if let Some(tx) = self.clients.lock().await.get(&client_id) {
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

    pub async fn broadcast_inbound(&self, envelope: &Envelope) -> Result<()> {
        // Push to receive buffer and get seq + timestamp
        let (seq, buffered_at_ms) = self.receive_buffer.lock().await.push(envelope.clone());

        let clients = self.clients.lock().await;
        let states = self.client_states.lock().await;

        // Track which consumers actually received the message
        let mut delivered_consumers: Vec<String> = Vec::new();

        for (client_id, tx) in clients.iter() {
            if let Some(state) = states.get(client_id) {
                // v1 clients (no hello) get legacy broadcast
                if state.version.is_none() {
                    let msg = DaemonReply::Inbound {
                        inbound: true,
                        envelope: envelope.clone(),
                    };
                    if let Ok(line) = serde_json::to_string(&msg) {
                        let _ = tx.try_send(Arc::from(line));
                    }
                    continue;
                }

                // v2+ clients only get messages if subscribed
                if let Some(filter) = &state.subscription
                    && filter.matches(&envelope.kind)
                    && seq > filter.replay_to_seq
                {
                    let event = DaemonReply::InboundEvent {
                        event: "inbound",
                        replay: false,
                        seq,
                        buffered_at_ms,
                        envelope: envelope.clone(),
                    };
                    if let Ok(line) = serde_json::to_string(&event)
                        && tx.try_send(Arc::from(line)).is_ok()
                    {
                        delivered_consumers.push(state.consumer.clone());
                    }
                }
            }
        }

        drop(states);
        drop(clients);

        // Only update delivered seq for consumers that actually received the message
        if !delivered_consumers.is_empty() {
            let mut buf = self.receive_buffer.lock().await;
            for consumer in delivered_consumers {
                buf.update_delivered_seq(&consumer, seq);
            }
        }

        Ok(())
    }
}
