mod broadcast;
mod commands;
mod hello_auth;

use std::collections::HashMap;
use std::sync::Arc;

use super::backend::IpcBackend;
use super::protocol::{DaemonReply, IpcCommand, IpcErrorCode};
use super::receive_buffer::ReceiveBuffer;
use crate::message::{Envelope, MessageKind};
use anyhow::Result;
use tokio::sync::Mutex;

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

impl ClientState {
    /// Returns true if the client has completed the hello handshake.
    fn has_hello(&self) -> bool {
        self.version.is_some()
    }

    /// Returns the negotiated version, defaulting to 1 for pre-hello clients.
    fn negotiated_version(&self) -> u32 {
        self.version.unwrap_or(1)
    }

    /// Returns true if the client negotiated v2+ semantics.
    fn is_v2_semantics(&self) -> bool {
        self.negotiated_version() >= 2
    }
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
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
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
            clock: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
            }),
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

        tracing::debug!(
            client_id,
            cmd = %command.cmd_name(),
            req_id = req_id.as_deref().unwrap_or("-"),
            consumer = %state.consumer,
            negotiated_version = state.version,
            "IPC command dispatched"
        );

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
        if is_v2_only_command && !state.is_v2_semantics() {
            let error = if state.has_hello() {
                IpcErrorCode::InvalidCommand
            } else {
                IpcErrorCode::HelloRequired
            };
            return Ok(DispatchResult {
                reply: DaemonReply::Error {
                    ok: false,
                    error,
                    req_id,
                },
                response_envelope: None,
                close: false,
            });
        }

        // Check if this client uses v2+ semantics (negotiated version >= 2)
        let is_v2_client = state.is_v2_semantics();

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
        // v1 clients (no hello or negotiated v1) bypass all auth checks
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
                        let error_code = if let Some(e) =
                            e.downcast_ref::<crate::daemon::command_handler::DaemonIpcError>()
                        {
                            match e {
                                crate::daemon::command_handler::DaemonIpcError::PeerNotFound => {
                                    IpcErrorCode::PeerNotFound
                                }
                                crate::daemon::command_handler::DaemonIpcError::PeerUnreachable => {
                                    IpcErrorCode::PeerUnreachable
                                }
                                crate::daemon::command_handler::DaemonIpcError::InvalidCommand(
                                    _,
                                ) => IpcErrorCode::InvalidCommand,
                            }
                        } else {
                            IpcErrorCode::InternalError
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
}
