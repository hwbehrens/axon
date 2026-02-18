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
    match spawn_multi_reply_server(root, vec![reply]) {
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
fn config_get_set_list_and_unset_roundtrip() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let root_str = root.path().to_str().expect("utf8 path");

    let get_initial =
        run_command(Command::new(&bin).args(["--state-root", root_str, "config", "name"]));
    assert_eq!(get_initial.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&get_initial.stdout)
            .trim()
            .is_empty()
    );

    let set_name =
        run_command(Command::new(&bin).args(["--state-root", root_str, "config", "name", "Alice"]));
    assert!(set_name.status.success());

    let get_name =
        run_command(Command::new(&bin).args(["--state-root", root_str, "config", "name"]));
    assert!(get_name.status.success());
    assert_eq!(String::from_utf8_lossy(&get_name.stdout).trim(), "Alice");

    let list = run_command(Command::new(&bin).args(["--state-root", root_str, "config", "--list"]));
    assert!(list.status.success());
    let list_stdout = String::from_utf8_lossy(&list.stdout);
    assert!(list_stdout.contains("name=Alice"));
    assert!(!list_stdout.contains("port="));

    let unset = run_command(Command::new(&bin).args([
        "--state-root",
        root_str,
        "config",
        "--unset",
        "name",
    ]));
    assert!(unset.status.success());

    let get_after_unset =
        run_command(Command::new(&bin).args(["--state-root", root_str, "config", "name"]));
    assert_eq!(get_after_unset.status.code(), Some(1));
}

#[test]
fn config_set_invalid_port_fails() {
    let bin = axon_bin();
    let root = tempdir().expect("tempdir");
    let root_str = root.path().to_str().expect("utf8 path");

    let output = run_command(Command::new(&bin).args([
        "--state-root",
        root_str,
        "config",
        "port",
        "not-a-number",
    ]));
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid port"));
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
