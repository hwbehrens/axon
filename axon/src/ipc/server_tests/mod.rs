// Common imports for all IPC server tests
use super::*;
use crate::ipc::IpcServerConfig;
use crate::message::MessageKind;
use serde_json::json;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

// Test modules
mod auth;
mod buffer;
mod subscribe;
mod v1_compat;
