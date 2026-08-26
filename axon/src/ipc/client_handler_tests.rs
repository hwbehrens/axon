//! Client-handler ingress contract tests (DEC-023).
//!
//! Pins the `req_id` ingress bound: an overlong `req_id` on a well-formed
//! command is rejected with `invalid_command` WITHOUT echoing the offending
//! value (an unbounded echo could produce reply frames past the line limit),
//! and malformed-command error lines always go through the shared outbound
//! encoder.

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::client_handler::handle_client;
use super::protocol::{CommandEvent, MAX_IPC_LINE_LENGTH, MAX_REQ_ID_BYTES};

struct TestClient {
    socket: BufReader<UnixStream>,
    cmd_rx: mpsc::Receiver<CommandEvent>,
    _handle: tokio::task::JoinHandle<()>,
}

async fn spawn_client() -> TestClient {
    let (client_sock, daemon_sock) = UnixStream::pair().expect("socket pair");
    let (out_tx, out_rx) = mpsc::channel::<std::sync::Arc<str>>(16);
    let (cmd_tx, cmd_rx) = mpsc::channel::<CommandEvent>(16);
    let cancel = CancellationToken::new();
    let handle = tokio::spawn(async move {
        let _ = handle_client(daemon_sock, 1, out_tx, out_rx, cmd_tx, cancel).await;
    });
    TestClient {
        socket: BufReader::new(client_sock),
        cmd_rx,
        _handle: handle,
    }
}

impl TestClient {
    async fn send(&mut self, line: &str) {
        self.socket
            .get_mut()
            .write_all(line.as_bytes())
            .await
            .expect("write command");
        self.socket
            .get_mut()
            .write_all(b"\n")
            .await
            .expect("write newline");
    }

    async fn read_reply(&mut self) -> String {
        let mut buf = Vec::new();
        let bytes = self
            .socket
            .read_until(b'\n', &mut buf)
            .await
            .expect("read reply");
        assert!(bytes > 0, "client disconnected before replying");
        let line = String::from_utf8(buf).expect("utf-8 reply");
        assert!(
            line.len() <= MAX_IPC_LINE_LENGTH,
            "reply frame ({} bytes incl. newline) exceeds the IPC line limit",
            line.len()
        );
        line
    }
}

#[tokio::test]
async fn overlong_req_id_on_valid_command_is_rejected_without_echo() {
    let mut client = spawn_client().await;

    let overlong = "r".repeat(MAX_REQ_ID_BYTES + 1);
    let command = format!("{{\"cmd\":\"peers\",\"req_id\":\"{overlong}\"}}");
    assert!(
        command.len() < MAX_IPC_LINE_LENGTH,
        "the command itself must be a legal frame"
    );
    client.send(&command).await;

    let reply = client.read_reply().await;
    let decoded: Value = serde_json::from_str(&reply).expect("json error reply");
    assert_eq!(decoded["ok"], serde_json::json!(false));
    assert_eq!(decoded["error"], "invalid_command");
    assert!(
        decoded.get("req_id").is_none(),
        "the offending overlong req_id must not be echoed"
    );
    assert!(
        client.cmd_rx.try_recv().is_err(),
        "the rejected command must not be dispatched"
    );
}

#[tokio::test]
async fn req_id_at_the_bound_is_dispatched_verbatim() {
    let mut client = spawn_client().await;

    let at_bound = "r".repeat(MAX_REQ_ID_BYTES);
    let command = format!("{{\"cmd\":\"peers\",\"req_id\":\"{at_bound}\"}}");
    client.send(&command).await;

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), client.cmd_rx.recv())
        .await
        .expect("command is dispatched within the deadline")
        .expect("command channel stays open");
    let dispatched_req_id = match &event.command {
        super::protocol::IpcCommand::Peers { req_id } => req_id.clone(),
        other => panic!("expected peers command, got {other:?}"),
    };
    assert_eq!(dispatched_req_id.as_deref(), Some(at_bound.as_str()));
}

#[tokio::test]
async fn malformed_command_with_overlong_req_id_errors_within_the_limit() {
    let mut client = spawn_client().await;

    // Valid JSON object, but not a valid command (unknown cmd), carrying an
    // overlong req_id: the error reply must stay within the line limit and
    // must not echo the offending value.
    let overlong = "r".repeat(60_000);
    let command = format!("{{\"cmd\":\"definitely_not_a_command\",\"req_id\":\"{overlong}\"}}");
    assert!(
        command.len() < MAX_IPC_LINE_LENGTH,
        "the malformed command must be a legal-size frame"
    );
    client.send(&command).await;

    let reply = client.read_reply().await;
    let decoded: Value = serde_json::from_str(&reply).expect("json error reply");
    assert_eq!(decoded["error"], "invalid_command");
    assert!(
        decoded.get("req_id").is_none(),
        "an overlong req_id on a malformed command must not be echoed"
    );
}

#[tokio::test]
async fn unparseable_line_with_overlong_req_id_errors_within_the_limit() {
    let mut client = spawn_client().await;

    // Not even valid JSON, but the req_id field is extractable: the echo is
    // dropped because it exceeds the bound.
    let overlong = "r".repeat(60_000);
    let line = format!("{{not-json \"req_id\":\"{overlong}\"}}");
    client.send(&line).await;

    let reply = client.read_reply().await;
    let decoded: Value = serde_json::from_str(&reply).expect("json error reply");
    assert_eq!(decoded["error"], "invalid_command");
    assert!(decoded.get("req_id").is_none());
}

#[test]
fn req_id_bound_is_small_enough_to_keep_error_replies_frameable() {
    // The largest terminal error reply is static text plus the echoed id:
    // with the bound, it can never approach the 64KB line limit.
    let longest_message = super::protocol::IpcErrorCode::InternalError.message().len();
    assert!(
        longest_message + MAX_REQ_ID_BYTES + 64 < MAX_IPC_LINE_LENGTH,
        "an error reply with a maximum-size echo must always fit the frame"
    );
}
