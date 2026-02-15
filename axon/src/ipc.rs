use anyhow::Result;
use std::path::Path;
use tokio::net::UnixListener;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum IpcCommand {
    #[serde(rename = "send")]
    Send {
        to: String,
        kind: String,
        payload: Value,
    },
    #[serde(rename = "peers")]
    Peers,
    #[serde(rename = "status")]
    Status,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
}

pub struct IpcServer {
    listener: UnixListener,
}

impl IpcServer {
    pub fn new(path: &Path) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self { listener })
    }

    pub async fn accept(&self) -> Result<tokio::net::UnixStream> {
        let (stream, _) = self.listener.accept().await?;
        Ok(stream)
    }
}
