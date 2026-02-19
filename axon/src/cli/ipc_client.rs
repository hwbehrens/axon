use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use axon::config::AxonPaths;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMode {
    Generic,
    Request,
}

pub fn is_unsolicited_event(decoded: &Value) -> bool {
    decoded.get("event").is_some()
}

pub fn daemon_reply_exit_code(response: &Value, mode: ResponseMode) -> ExitCode {
    if response.get("ok") == Some(&json!(false)) {
        if response.get("error").and_then(Value::as_str) == Some("timeout") {
            return ExitCode::from(3);
        }
        return ExitCode::from(2);
    }

    if mode == ResponseMode::Request
        && response
            .get("response")
            .and_then(|inner| inner.get("kind"))
            .and_then(Value::as_str)
            == Some("error")
    {
        return ExitCode::from(2);
    }

    ExitCode::SUCCESS
}

pub fn render_json(value: &Value) -> Result<String> {
    serde_json::to_string_pretty(value).context("failed to encode response output")
}

pub async fn send_ipc(paths: &AxonPaths, command: Value) -> Result<Value> {
    let line = serde_json::to_string(&command).context("failed to serialize IPC command")?;
    if line.len() > axon::ipc::MAX_IPC_LINE_LENGTH {
        anyhow::bail!(
            "IPC command size ({} bytes) exceeds the 64KB limit",
            line.len()
        );
    }

    tracing::debug!(socket = %paths.socket.display(), "connecting to daemon IPC socket");

    let mut stream = UnixStream::connect(&paths.socket).await.with_context(|| {
        format!(
            "failed to connect to daemon socket: {}. Is the daemon running?",
            paths.socket.display()
        )
    })?;

    tracing::debug!(cmd_bytes = line.len(), "sending IPC command");

    stream
        .write_all(line.as_bytes())
        .await
        .context("failed to write IPC command")?;
    stream
        .write_all(b"\n")
        .await
        .context("failed to write IPC newline")?;

    let mut reader = BufReader::new(stream);
    loop {
        let mut response = Vec::new();
        let bytes = reader
            .read_until(b'\n', &mut response)
            .await
            .context("failed to read IPC response")?;
        if bytes == 0 {
            return Err(anyhow!(
                "daemon closed connection without a command response"
            ));
        }

        if response.last() == Some(&b'\n') {
            response.pop();
        }

        let line = std::str::from_utf8(&response)
            .context("failed to decode IPC response as UTF-8")?
            .trim();
        if line.is_empty() {
            continue;
        }

        let decoded: Value = serde_json::from_str(line).context("failed to decode IPC response")?;
        if is_unsolicited_event(&decoded) {
            tracing::debug!("skipping unsolicited IPC event");
            continue;
        }
        tracing::debug!("received IPC command response");
        return Ok(decoded);
    }
}

#[cfg(test)]
#[path = "ipc_client_tests.rs"]
mod tests;
