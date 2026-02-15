// Common imports for all IPC server tests
use super::*;
use crate::ipc::{IpcErrorCode, IpcServerConfig};
use crate::message::MessageKind;
use serde_json::json;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Wait until the server has registered the expected number of clients.
async fn wait_for_clients(server: &IpcServer, expected: usize) {
    for _ in 0..100 {
        if server.client_count().await >= expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!(
        "timed out waiting for {} clients (got {})",
        expected,
        // can't await here but the panic message is best-effort
        expected
    );
}

// Test modules
mod auth;
mod buffer;
mod subscribe;
mod v1_compat;
