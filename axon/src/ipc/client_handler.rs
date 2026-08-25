use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::protocol::{CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, MAX_IPC_LINE_LENGTH};

/// Maximum time the handler may spend draining an overlong line before the
/// client is closed. The drain exists so the queued `command_too_large`
/// error survives the connection close (an abrupt close with unread inbound
/// data sends RST and discards it); without a deadline a client that pauses
/// mid-line would hold one of the bounded IPC client slots indefinitely,
/// and server shutdown could not interrupt it.
pub(super) const IPC_OVERLONG_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

fn build_error_line(error: IpcErrorCode, req_id: Option<String>) -> Arc<str> {
    Arc::from(
        serde_json::to_string(&DaemonReply::Error {
            ok: false,
            message: error.message().to_string(),
            error,
            req_id,
        })
        .expect("IPC error serialization"),
    )
}

fn extract_req_id(line: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(line).ok()?;
    parsed
        .get("req_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn try_queue_error(
    out_tx: &mpsc::Sender<Arc<str>>,
    error: IpcErrorCode,
    req_id: Option<String>,
) -> bool {
    out_tx.try_send(build_error_line(error, req_id)).is_ok()
}

pub(super) async fn handle_client(
    socket: UnixStream,
    client_id: u64,
    out_tx: mpsc::Sender<Arc<str>>,
    mut out_rx: mpsc::Receiver<Arc<str>>,
    cmd_tx: mpsc::Sender<CommandEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    #[derive(Clone, Copy)]
    enum WriterCloseMode {
        Immediate,
        FlushQueued,
    }

    let (read_half, mut write_half) = socket.into_split();
    let writer_cancel = cancel.clone();
    let (writer_close_tx, mut writer_close_rx) = oneshot::channel::<WriterCloseMode>();

    let mut writer_handle = tokio::spawn(async move {
        let mut close_mode = WriterCloseMode::Immediate;
        loop {
            tokio::select! {
                _ = writer_cancel.cancelled() => break,
                mode = &mut writer_close_rx => {
                    close_mode = mode.unwrap_or(WriterCloseMode::Immediate);
                    break;
                }
                maybe_line = out_rx.recv() => {
                    let Some(line) = maybe_line else {
                        break;
                    };
                    if write_half.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if write_half.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
            }
        }

        if matches!(close_mode, WriterCloseMode::FlushQueued) {
            while let Ok(line) = out_rx.try_recv() {
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        }

        let _ = write_half.shutdown().await;
    });

    let mut reader = BufReader::new(read_half);
    let mut buf = Vec::with_capacity(MAX_IPC_LINE_LENGTH + 1);
    let mut writer_close_mode = WriterCloseMode::Immediate;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        buf.clear();
        let mut found_newline = false;
        let mut exceeded = false;

        loop {
            let available = tokio::select! {
                _ = cancel.cancelled() => break,
                read_result = reader.fill_buf() => read_result.context("failed reading IPC")?,
            };
            if available.is_empty() {
                break; // EOF
            }

            if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                let needed = pos;
                // Spec counts the trailing newline in the 65,536-byte bound.
                if buf.len() + needed + 1 > MAX_IPC_LINE_LENGTH {
                    exceeded = true;
                    reader.consume(pos + 1);
                    break;
                }
                buf.extend_from_slice(&available[..pos]);
                reader.consume(pos + 1);
                found_newline = true;
                break;
            } else {
                let len = available.len();
                // Reserve one byte for the newline that must eventually end
                // the line (spec: 65,536 bytes including the newline).
                if buf.len() + len + 1 > MAX_IPC_LINE_LENGTH {
                    exceeded = true;
                    reader.consume(len);
                    break;
                }
                buf.extend_from_slice(available);
                reader.consume(len);
            }
        }

        if exceeded {
            if try_queue_error(&out_tx, IpcErrorCode::CommandTooLarge, None) {
                writer_close_mode = WriterCloseMode::FlushQueued;
            }
            // Drain the remainder of the overlong line (bounded in bytes and
            // in time, and interruptible by cancellation) before closing:
            // closing with unread inbound data sends RST, which would
            // discard the queued error reply before the client can read it.
            let mut drained = 0usize;
            let deadline =
                tokio::time::sleep_until(tokio::time::Instant::now() + IPC_OVERLONG_DRAIN_TIMEOUT);
            tokio::pin!(deadline);
            loop {
                let available_len = tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = &mut deadline => {
                        tracing::warn!(client_id, "timed out draining overlong IPC line");
                        break;
                    }
                    read_result = reader.fill_buf() => match read_result {
                        Ok([]) => break, // EOF
                        Ok(chunk) => {
                            let newline = chunk.iter().position(|&b| b == b'\n');
                            match newline {
                                Some(pos) => {
                                    reader.consume(pos + 1);
                                    break;
                                }
                                None => chunk.len(),
                            }
                        }
                        Err(_) => break,
                    }
                };
                drained += available_len;
                reader.consume(available_len);
                if drained > MAX_IPC_LINE_LENGTH {
                    // Hostile never-terminated stream: stop reading.
                    break;
                }
            }
            break; // Close connection — command boundary was restored above
        }

        if !found_newline {
            break; // EOF
        }
        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                if !try_queue_error(&out_tx, IpcErrorCode::InvalidCommand, None) {
                    break;
                }
                continue;
            }
        };
        match serde_json::from_str::<IpcCommand>(line) {
            Ok(command) => {
                cmd_tx
                    .send(CommandEvent { client_id, command })
                    .await
                    .map_err(|_| anyhow::anyhow!("daemon command channel closed"))?;
            }
            Err(_err) => {
                if !try_queue_error(&out_tx, IpcErrorCode::InvalidCommand, extract_req_id(line)) {
                    break;
                }
            }
        }
    }

    let _ = writer_close_tx.send(writer_close_mode);
    if tokio::time::timeout(std::time::Duration::from_secs(1), &mut writer_handle)
        .await
        .is_err()
    {
        writer_handle.abort();
        tracing::warn!(
            client_id,
            "timed out waiting for IPC client writer shutdown"
        );
    }

    cancel.cancel();
    Ok(())
}
