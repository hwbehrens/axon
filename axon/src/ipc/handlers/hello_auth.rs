use anyhow::Result;
use tracing::{debug, info};

use super::{ClientState, IPC_VERSION, IpcHandlers, MAX_CONSUMER_LEN};
use crate::ipc::protocol::{DaemonReply, IpcErrorCode};

impl IpcHandlers {
    pub(super) async fn handle_hello(
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

    pub(super) async fn handle_auth(
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
                debug!(client_id, "IPC auth failed: malformed token");
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
}
