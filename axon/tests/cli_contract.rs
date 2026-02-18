use std::fs;
use std::io::{BufRead, BufReader, Error, ErrorKind, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::JoinHandle;

use axon::peer_token;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tempfile::tempdir;

const VALID_AGENT_ID: &str = "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VALID_AGENT_ID_UPPER: &str = "ED25519.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn axon_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_axon") {
        return PathBuf::from(bin);
    }

    let current = std::env::current_exe().expect("resolve current test executable");
    let debug_dir = current
        .parent()
        .and_then(Path::parent)
        .expect("resolve target debug dir");
    let fallback = if cfg!(windows) {
        debug_dir.join("axon.exe")
    } else {
        debug_dir.join("axon")
    };
    assert!(
        fallback.exists(),
        "failed to locate axon binary via CARGO_BIN_EXE_axon and fallback path {}",
        fallback.display()
    );
    fallback
}

fn run_command(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to execute axon binary")
}

fn spawn_multi_reply_server(
    root: &Path,
    replies: Vec<Value>,
) -> std::result::Result<JoinHandle<Value>, Error> {
    fs::create_dir_all(root).expect("create root");
    let socket_path = root.join("axon.sock");
    let _ = fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;

    Ok(std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        let mut line = String::new();
        {
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            reader.read_line(&mut line).expect("read command line");
        }

        for reply in replies {
            let payload = serde_json::to_string(&reply).expect("serialize reply");
            stream
                .write_all(payload.as_bytes())
                .expect("write reply payload");
            stream.write_all(b"\n").expect("write reply newline");
        }

        serde_json::from_str(line.trim()).expect("decode command JSON")
    }))
}

fn require_socket_server(root: &Path, reply: Value) -> Option<JoinHandle<Value>> {
    require_socket_server_with_replies(root, vec![reply])
}

fn require_socket_server_with_replies(
    root: &Path,
    replies: Vec<Value>,
) -> Option<JoinHandle<Value>> {
    match spawn_multi_reply_server(root, replies) {
        Ok(server) => Some(server),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping socket-dependent test: unix socket bind not permitted in this environment"
            );
            None
        }
        Err(err) => panic!("failed to start unix socket server: {err}"),
    }
}

#[test]
fn command_reply_ignores_unsolicited_inbound_event_lines() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let Some(server) = require_socket_server_with_replies(
        root.path(),
        vec![
            json!({
                "event": "inbound",
                "from": VALID_AGENT_ID,
                "envelope": {
                    "id": "550e8400-e29b-41d4-a716-446655440000",
                    "kind": "message",
                    "payload": {"data": {"hello": "world"}}
                }
            }),
            json!({
                "ok": true,
                "uptime_secs": 7,
                "peers_connected": 0,
                "messages_sent": 1,
                "messages_received": 2
            }),
        ],
    ) else {
        return;
    };

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "status",
    ]));
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"ok\": true"));
    assert!(stdout.contains("\"uptime_secs\": 7"));
    assert!(!stdout.contains("\"event\": \"inbound\""));

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "status");
}

#[test]
fn version_flags_print_version_and_exit_zero() {
    let bin = axon_bin();
    let expected = env!("CARGO_PKG_VERSION");

    let long = run_command(Command::new(&bin).arg("--version"));
    assert!(long.status.success());
    assert!(String::from_utf8_lossy(&long.stdout).contains(expected));

    let short = run_command(Command::new(&bin).arg("-V"));
    assert!(short.status.success());
    assert!(String::from_utf8_lossy(&short.stdout).contains(expected));
}

#[test]
fn verbose_flag_is_listed_in_help() {
    let bin = axon_bin();
    let output = run_command(Command::new(&bin).arg("--help"));
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--verbose"));
    assert!(stdout.contains("-v"));
}

#[test]
fn invalid_agent_id_rejected_at_cli_boundary() {
    let bin = axon_bin();
    let output = run_command(Command::new(&bin).args(["send", "banana", "hello"]));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid agent_id"));
}

#[test]
fn notify_malformed_json_like_payload_fails_fast() {
    let bin = axon_bin();
    let output = run_command(Command::new(&bin).args(["notify", VALID_AGENT_ID, "{\"x\":"]));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("appears JSON-like but is invalid"));
}

#[test]
fn notify_text_override_sends_literal_string_payload() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let Some(server) = require_socket_server(root.path(), json!({"ok": true, "msg_id": "x"}))
    else {
        return;
    };

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "notify",
        "--text",
        VALID_AGENT_ID,
        "{\"x\":",
    ]));
    assert!(output.status.success());

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "send");
    assert_eq!(command["kind"], "message");
    assert_eq!(command["payload"]["data"], "{\"x\":");
}

#[test]
fn uppercase_agent_id_is_canonicalized_before_send() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let Some(server) = require_socket_server(root.path(), json!({"ok": true, "msg_id": "x"}))
    else {
        return;
    };

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "send",
        VALID_AGENT_ID_UPPER,
        "hello",
    ]));
    assert!(output.status.success());

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "send");
    assert_eq!(command["to"], VALID_AGENT_ID);
}

#[test]
fn root_flag_aliases_route_client_commands_to_same_socket_contract() {
    let bin = axon_bin();
    for flag in ["--state-root", "--state", "--root"] {
        let root = tempdir().expect("tempdir");
        let Some(server) = require_socket_server(
            root.path(),
            json!({
                "ok": true,
                "uptime_secs": 1,
                "peers_connected": 0,
                "messages_sent": 0,
                "messages_received": 0
            }),
        ) else {
            return;
        };

        let output = run_command(Command::new(&bin).args([
            flag,
            root.path().to_str().expect("utf8 path"),
            "status",
        ]));
        assert!(output.status.success(), "{flag} should succeed");

        let command = server.join().expect("server thread");
        assert_eq!(command["cmd"], "status");
    }
}

#[test]
fn axon_root_env_routes_client_command_without_flag() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let Some(server) = require_socket_server(root.path(), json!({"ok": true, "peers": []})) else {
        return;
    };

    let output = run_command(
        Command::new(&bin)
            .arg("peers")
            .env("AXON_ROOT", root.path().to_str().expect("utf8 path")),
    );
    assert!(output.status.success());

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "peers");
}

#[test]
fn send_peer_not_found_returns_exit_code_two() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let Some(server) = require_socket_server(
        root.path(),
        json!({"ok": false, "error": "peer_not_found", "message": "peer_not_found"}),
    ) else {
        return;
    };

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "send",
        VALID_AGENT_ID,
        "hello",
    ]));
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"ok\": false"));

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "send");
    assert_eq!(command["to"], VALID_AGENT_ID);
}

#[test]
fn daemon_error_line_is_reported_instead_of_generic_eof_error() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let Some(server) = require_socket_server(
        root.path(),
        json!({
            "ok": false,
            "error": "command_too_large",
            "message": "IPC command exceeds 64KB limit"
        }),
    ) else {
        return;
    };

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "status",
    ]));
    assert_eq!(output.status.code(), Some(2));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"error\": \"command_too_large\""));
    assert!(stdout.contains("IPC command exceeds 64KB limit"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("daemon closed connection without a command response"),
        "CLI should surface daemon error response before EOF"
    );

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "status");
}

#[test]
fn whoami_uses_ipc_while_identity_is_local() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let root_str = root.path().to_str().expect("utf8 path");

    let Some(whoami_server) = require_socket_server(
        root.path(),
        json!({
            "ok": true,
            "agent_id": "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "public_key": "Zm9v",
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": 1
        }),
    ) else {
        return;
    };

    let whoami = run_command(Command::new(&bin).args(["--state-root", root_str, "whoami"]));
    assert!(whoami.status.success());
    assert!(
        String::from_utf8_lossy(&whoami.stdout)
            .contains("ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    let whoami_cmd = whoami_server.join().expect("server thread");
    assert_eq!(whoami_cmd["cmd"], "whoami");

    assert!(
        !root.path().join("identity.key").exists(),
        "whoami should not require local identity files"
    );

    let identity = run_command(Command::new(&bin).args(["--state-root", root_str, "identity"]));
    assert!(identity.status.success());
    assert!(root.path().join("identity.key").exists());
    assert!(root.path().join("identity.pub").exists());
}

#[test]
fn identity_default_outputs_peer_uri_and_json_flag_expands_fields() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let root_str = root.path().to_str().expect("utf8 path");

    let uri_out = run_command(Command::new(&bin).args(["--state-root", root_str, "identity"]));
    assert!(uri_out.status.success());
    let uri = String::from_utf8_lossy(&uri_out.stdout);
    assert!(uri.trim().starts_with("axon://"));

    let json_out =
        run_command(Command::new(&bin).args(["--state-root", root_str, "identity", "--json"]));
    assert!(json_out.status.success());
    let parsed: Value = serde_json::from_slice(&json_out.stdout).expect("identity json");

    assert!(
        parsed["agent_id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("ed25519.")
    );
    assert!(parsed["public_key"].as_str().unwrap_or_default().len() > 10);
    assert!(parsed["addr"].as_str().is_some());
    assert!(parsed["port"].as_u64().is_some());
    assert!(
        parsed["uri"]
            .as_str()
            .unwrap_or_default()
            .starts_with("axon://")
    );
}

#[test]
fn legacy_raw_identity_key_is_auto_migrated() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    fs::create_dir_all(root.path()).expect("create root");
    fs::write(root.path().join("identity.key"), [7u8; 32]).expect("write raw key");

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root.path().to_str().expect("utf8 path"),
        "identity",
    ]));
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().starts_with("axon://"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(
        "Notice: migrated identity.key from legacy raw format to base64. Agent ID unchanged."
    ));

    let migrated = fs::read_to_string(root.path().join("identity.key")).expect("read migrated key");
    let decoded = STANDARD
        .decode(migrated.trim())
        .expect("migrated identity.key should be base64");
    assert_eq!(decoded.len(), 32);
}

#[test]
fn connect_writes_config_and_sends_add_peer_ipc() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let root_str = root.path().to_str().expect("utf8 path");

    let pubkey = STANDARD.encode([9u8; 32]);
    let token = peer_token::encode(&pubkey, "127.0.0.1:7710").expect("token");
    let expected_agent = peer_token::derive_agent_id_from_pubkey_base64(&pubkey)
        .expect("derive")
        .to_string();

    let Some(server) =
        require_socket_server(root.path(), json!({"ok": true, "agent_id": expected_agent}))
    else {
        return;
    };

    let output =
        run_command(Command::new(&bin).args(["--state-root", root_str, "connect", &token]));
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Added peer"));

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "add_peer");
    assert_eq!(command["addr"], "127.0.0.1:7710");
    assert_eq!(command["pubkey"], pubkey);

    let saved = fs::read_to_string(root.path().join("config.yaml")).expect("config saved");
    assert!(saved.contains("127.0.0.1:7710"));
}

#[test]
fn connect_returns_error_when_hotload_fails_after_config_write() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let root_str = root.path().to_str().expect("utf8 path");

    let pubkey = STANDARD.encode([11u8; 32]);
    let token = peer_token::encode(&pubkey, "127.0.0.1:7720").expect("token");

    let Some(server) = require_socket_server(
        root.path(),
        json!({"ok": false, "error": "invalid_command", "message": "bad peer"}),
    ) else {
        return;
    };

    let output =
        run_command(Command::new(&bin).args(["--state-root", root_str, "connect", &token]));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("peer saved to"));
    assert!(stderr.contains("hot-load failed"));

    let saved = fs::read_to_string(root.path().join("config.yaml")).expect("config saved");
    assert!(saved.contains("127.0.0.1:7720"));

    let command = server.join().expect("server thread");
    assert_eq!(command["cmd"], "add_peer");
}
