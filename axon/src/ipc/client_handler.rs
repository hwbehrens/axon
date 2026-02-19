use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::protocol::{CommandEvent, DaemonReply, IpcCommand, MAX_IPC_LINE_LENGTH};

static INVALID_COMMAND_LINE: LazyLock<Arc<str>> = LazyLock::new(|| {
    let error = super::protocol::IpcErrorCode::InvalidCommand;
    Arc::from(
        serde_json::to_string(&DaemonReply::Error {
            ok: false,
            message: error.message(),
            error,
            req_id: None,
        })
        .expect("static error serialization"),
    )
});

static COMMAND_TOO_LARGE_LINE: LazyLock<Arc<str>> = LazyLock::new(|| {
    let error = super::protocol::IpcErrorCode::CommandTooLarge;
    Arc::from(
        serde_json::to_string(&DaemonReply::Error {
            ok: false,
            message: error.message(),
            error,
            req_id: None,
        })
        .expect("static error serialization"),
    )
});

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
                if buf.len() + needed > MAX_IPC_LINE_LENGTH {
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
                if buf.len() + len > MAX_IPC_LINE_LENGTH {
                    exceeded = true;
                    reader.consume(len);
                    break;
                }
                buf.extend_from_slice(available);
                reader.consume(len);
            }
        }

        if exceeded {
            let _ = out_tx.send(COMMAND_TOO_LARGE_LINE.clone()).await;
            writer_close_mode = WriterCloseMode::FlushQueued;
            break; // Close connection — can't reliably find next command boundary
        }

        if !found_newline {
            break; // EOF
        }
        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(_) => {
                let _ = out_tx.try_send(INVALID_COMMAND_LINE.clone());
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
                let _ = out_tx.try_send(INVALID_COMMAND_LINE.clone());
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
