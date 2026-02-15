use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::protocol::{DaemonReply, IpcCommand, WhoamiInfo};
use super::receive_buffer::ReceiveBuffer;
use crate::message::{Envelope, MessageKind};

const IPC_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Client state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SubscriptionFilter {
    pub kinds: Option<Vec<MessageKind>>,
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
        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);

        // Check if this is a v2 client (has done hello)
        let is_v2_client = state.version.is_some();

        // Only require auth if the client has done hello (v2+)
        // v1 clients (version=None) bypass all auth checks
        if is_v2_client && !state.authenticated {
            // Once a client has done hello, require auth for all commands
            // except hello, auth, and status
            let requires_auth = !matches!(
                command,
                IpcCommand::Hello { .. } | IpcCommand::Auth { .. } | IpcCommand::Status
            );

            if requires_auth {
                return Ok(DaemonReply::Error {
                    ok: false,
                    error: "auth_required".to_string(),
                });
            }
        }

        drop(states);

        match command {
            IpcCommand::Hello { version } => self.handle_hello(client_id, version).await,
            IpcCommand::Auth { token } => self.handle_auth(client_id, token).await,
            IpcCommand::Whoami => self.handle_whoami().await,
            IpcCommand::Inbox {
                limit,
                since,
                kinds,
            } => {
                self.handle_inbox(limit, since.as_deref(), kinds.as_deref())
                    .await
            }
            IpcCommand::Ack { ids } => self.handle_ack(ids).await,
            IpcCommand::Subscribe { since, kinds } => {
                self.handle_subscribe(client_id, since.as_deref(), kinds)
                    .await
            }
            // v1 commands handled elsewhere
            _ => Ok(DaemonReply::Error {
                ok: false,
                error: "command must be handled by daemon".to_string(),
            }),
        }
    }

    async fn handle_hello(&self, client_id: u64, version: u32) -> Result<DaemonReply> {
        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);
        state.version = Some(version);
        drop(states);

        debug!(client_id, version, "IPC hello handshake");

        Ok(DaemonReply::Hello {
            ok: true,
            version: IPC_VERSION,
            agent_id: self.config.agent_id.clone(),
            features: vec![
                "auth".to_string(),
                "buffer".to_string(),
                "subscribe".to_string(),
            ],
        })
    }

    async fn handle_auth(&self, client_id: u64, token: String) -> Result<DaemonReply> {
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
            });
        }

        // Check token if configured
        if let Some(expected_token) = &self.config.token {
            // Validate token format: must be exactly 64 hex characters
            if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
                warn!(client_id, "IPC auth failed: malformed token");
                return Ok(DaemonReply::Error {
                    ok: false,
                    error: "auth_failed".to_string(),
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
                })
            } else {
                warn!(client_id, "IPC auth failed: invalid token");
                Ok(DaemonReply::Error {
                    ok: false,
                    error: "auth_failed".to_string(),
                })
            }
        } else {
            // No token configured, accept
            state.authenticated = true;
            info!(client_id, method = "token", "IPC client authenticated");
            Ok(DaemonReply::Auth {
                ok: true,
                auth: "accepted".to_string(),
            })
        }
    }

    async fn handle_whoami(&self) -> Result<DaemonReply> {
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
        })
    }

    async fn handle_inbox(
        &self,
        limit: usize,
        since: Option<&str>,
        kinds: Option<&[MessageKind]>,
    ) -> Result<DaemonReply> {
        let (messages, has_more) = self.receive_buffer.lock().await.fetch(limit, since, kinds);

        Ok(DaemonReply::Inbox {
            ok: true,
            messages,
            has_more,
        })
    }

    async fn handle_ack(&self, ids: Vec<Uuid>) -> Result<DaemonReply> {
        let acked = self.receive_buffer.lock().await.ack(&ids);

        Ok(DaemonReply::Ack { ok: true, acked })
    }

    async fn handle_subscribe(
        &self,
        client_id: u64,
        since: Option<&str>,
        kinds: Option<Vec<MessageKind>>,
    ) -> Result<DaemonReply> {
        let mut states = self.client_states.lock().await;
        let state = states.entry(client_id).or_insert_with(ClientState::default);

        let filter = SubscriptionFilter {
            kinds: kinds.clone(),
        };
        debug!(client_id, kinds = ?filter.kinds, "IPC client subscribed");
        state.subscription = Some(filter.clone());
        drop(states);

        // Replay buffered messages if requested
        let mut replayed = 0;
        if since.is_some() {
            let (messages, _) =
                self.receive_buffer
                    .lock()
                    .await
                    .fetch(usize::MAX, since, kinds.as_deref());

            if let Some(tx) = self.clients.lock().await.get(&client_id) {
                for msg in messages {
                    let reply = DaemonReply::Inbound {
                        inbound: true,
                        envelope: msg.envelope,
                    };
                    if let Ok(json) = serde_json::to_string(&reply) {
                        let _ = tx.try_send(Arc::from(json));
                        replayed += 1;
                    }
                }
            }
        }

        Ok(DaemonReply::Subscribe {
            ok: true,
            subscribed: true,
            replayed,
        })
    }

    pub async fn broadcast_inbound(&self, envelope: &Envelope) -> Result<()> {
        // Always push to receive buffer
        self.receive_buffer.lock().await.push(envelope.clone());

        // Deliver to connected clients based on their version and subscription
        let msg = DaemonReply::Inbound {
            inbound: true,
            envelope: envelope.clone(),
        };
        let line: Arc<str> =
            Arc::from(serde_json::to_string(&msg).context("failed to serialize inbound message")?);

        let clients = self.clients.lock().await;
        let states = self.client_states.lock().await;

        for (client_id, tx) in clients.iter() {
            if let Some(state) = states.get(client_id) {
                // v1 clients (no hello) get everything (legacy broadcast)
                if state.version.is_none() {
                    let _ = tx.try_send(line.clone());
                    continue;
                }

                // v2+ clients only get messages if subscribed
                if let Some(filter) = &state.subscription
                    && filter.matches(&envelope.kind)
                {
                    let _ = tx.try_send(line.clone());
                }
            }
        }

        Ok(())
    }
}
