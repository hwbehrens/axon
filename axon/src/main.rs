use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{Instant, timeout};
use tracing_subscriber::EnvFilter;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;

#[derive(Debug, Parser)]
#[command(name = "axon", about = "AXON — Agent eXchange Over Network")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the daemon (runs in foreground).
    Daemon {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        enable_mdns: bool,
        /// Override the derived agent_id (for testing/aliasing).
        #[arg(long)]
        agent_id: Option<String>,
    },
    /// Send a query to another agent.
    Send {
        agent_id: String,
        message: String,
    },
    /// Delegate a task to another agent.
    Delegate {
        agent_id: String,
        task: String,
    },
    /// Send a notification (fire-and-forget).
    Notify {
        agent_id: String,
        topic: String,
        data: String,
    },
    /// Send a ping to another agent.
    Ping {
        agent_id: String,
    },
    /// Discover another agent's capabilities.
    Discover {
        agent_id: String,
    },
    /// Cancel a previously delegated task.
    Cancel {
        agent_id: String,
        #[arg(long, name = "ref")]
        ref_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// List discovered and connected peers.
    Peers,
    /// Show daemon status.
    Status,
    /// Print this agent's identity.
    Identity,
    /// Print example interactions.
    Examples,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon {
            port,
            enable_mdns,
            agent_id,
        } => {
            run_daemon(DaemonOptions {
                port,
                enable_mdns,
                axon_root: None,
                agent_id,
                cancel: None,
            })
            .await?;
        }
        Commands::Send { agent_id, message } => {
            let payload = json!({
                "question": message,
                "domain": "meta.query",
                "max_tokens": 200,
                "deadline_ms": 30000
            });
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "query", "payload": payload}),
                true,
            )
            .await?;
            println!("{line}");
        }
        Commands::Delegate { agent_id, task } => {
            let payload = json!({
                "task": task,
                "priority": "normal",
                "report_back": true,
                "deadline_ms": 60000
            });
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "delegate", "payload": payload}),
                true,
            )
            .await?;
            println!("{line}");
        }
        Commands::Notify {
            agent_id,
            topic,
            data,
        } => {
            let parsed_data =
                serde_json::from_str::<Value>(&data).unwrap_or_else(|_| json!(data));
            let payload = json!({
                "topic": topic,
                "data": parsed_data,
                "importance": "low"
            });
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "notify", "payload": payload}),
                false,
            )
            .await?;
            println!("{line}");
        }
        Commands::Ping { agent_id } => {
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "ping", "payload": {}}),
                true,
            )
            .await?;
            println!("{line}");
        }
        Commands::Discover { agent_id } => {
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "discover", "payload": {}}),
                true,
            )
            .await?;
            println!("{line}");
        }
        Commands::Cancel {
            agent_id,
            ref_id,
            reason,
        } => {
            let payload = json!({"reason": reason});
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "cancel", "payload": payload, "ref": ref_id}),
                true,
            )
            .await?;
            println!("{line}");
        }
        Commands::Peers => {
            let line = send_ipc(json!({"cmd": "peers"}), false).await?;
            println!("{line}");
        }
        Commands::Status => {
            let line = send_ipc(json!({"cmd": "status"}), false).await?;
            println!("{line}");
        }
        Commands::Identity => {
            let paths = AxonPaths::discover()?;
            let identity = Identity::load_or_generate(&paths)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "agent_id": identity.agent_id(),
                    "public_key": identity.public_key_base64(),
                }))
                .context("failed to encode identity output")?
            );
        }
        Commands::Examples => {
            print_annotated_examples();
        }
    }

    Ok(())
}

fn print_annotated_examples() {
    println!(r#"AXON — Complete annotated example interaction
==============================================

LLMs learn from examples faster than from specifications.
Below is a full hello → discover → query → delegate → cancel → notify sequence.

Agent IDs used:
  Alice: a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4  (lower — initiates connection)
  Bob:   f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

──────────────────────────────────────────────
Step 0: Start the daemon
──────────────────────────────────────────────
$ axon daemon --port 7100

  INFO starting AXON daemon agent_id=a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4 port=7100

  (The daemon binds QUIC on 0.0.0.0:7100, creates ~/.axon/axon.sock for IPC,
   and begins connecting to any peers listed in ~/.axon/config.toml.)

──────────────────────────────────────────────
Step 1: List known peers
──────────────────────────────────────────────
$ axon peers

  IPC sent:     {{"cmd":"peers"}}
  IPC response: {{
    "ok": true,
    "peers": [
      {{
        "id": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "addr": "192.168.1.42:7100",
        "status": "connected",
        "rtt_ms": 1.23,
        "source": "static"
      }}
    ]
  }}

──────────────────────────────────────────────
Step 2: Discover peer capabilities
──────────────────────────────────────────────
$ axon discover f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

  IPC sent:     {{"cmd":"send","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"discover","payload":{{}}}}
  IPC ack:      {{"ok":true,"msg_id":"550e8400-e29b-41d4-a716-446655440000"}}
  Wire message: {{
    "v": 1,
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "from": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
    "to": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
    "ts": 1771108000000,
    "kind": "discover",
    "payload": {{}}
  }}
  Wire response: {{
    "v": 1,
    "id": "660e8400-e29b-41d4-a716-446655440001",
    "from": "f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
    "to": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
    "ts": 1771108000050,
    "kind": "capabilities",
    "ref": "550e8400-e29b-41d4-a716-446655440000",
    "payload": {{
      "agent_name": "Bob's Research Assistant",
      "domains": ["web_search", "summarization"],
      "tools": ["web_search", "pdf_reader"],
      "max_concurrent_tasks": 4
    }}
  }}

──────────────────────────────────────────────
Step 3: Send a query
──────────────────────────────────────────────
$ axon send f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 "What is the capital of France?"

  IPC sent:     {{"cmd":"send","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"query","payload":{{"question":"What is the capital of France?","domain":"meta.query","max_tokens":200,"deadline_ms":30000}}}}
  Wire response: {{
    "v": 1,
    "kind": "response",
    "ref": "<msg_id>",
    "payload": {{
      "data": {{"answer": "Paris"}},
      "summary": "The capital of France is Paris.",
      "tokens_used": 12,
      "truncated": false
    }}
  }}

──────────────────────────────────────────────
Step 4: Delegate a task
──────────────────────────────────────────────
$ axon delegate f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 "Summarize today's tech news"

  IPC sent:     {{"cmd":"send","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"delegate","payload":{{"task":"Summarize today's tech news","priority":"normal","report_back":true,"deadline_ms":60000}}}}
  Wire response (immediate ack): {{
    "v": 1,
    "kind": "ack",
    "ref": "<msg_id>",
    "payload": {{"accepted": true, "estimated_ms": 15000}}
  }}
  Wire response (later, via unidirectional stream): {{
    "v": 1,
    "kind": "result",
    "ref": "<msg_id>",
    "payload": {{
      "status": "completed",
      "outcome": "Here are today's top tech stories: ...",
      "data": {{"articles": 5}}
    }}
  }}

──────────────────────────────────────────────
Step 5: Cancel a delegated task
──────────────────────────────────────────────
$ axon cancel f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 --ref 550e8400-e29b-41d4-a716-446655440000 --reason "No longer needed"

  IPC sent:     {{"cmd":"send","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"cancel","payload":{{"reason":"No longer needed"}},"ref":"550e8400-e29b-41d4-a716-446655440000"}}
  Wire response: {{
    "v": 1,
    "kind": "ack",
    "ref": "<msg_id>",
    "payload": {{"accepted": true}}
  }}

──────────────────────────────────────────────
Step 6: Send a notification (fire-and-forget)
──────────────────────────────────────────────
$ axon notify f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 meta.status '{{"state":"ready"}}'

  IPC sent:     {{"cmd":"send","to":"f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"notify","payload":{{"topic":"meta.status","data":{{"state":"ready"}},"importance":"low"}}}}
  IPC ack:      {{"ok":true,"msg_id":"..."}}
  (No wire response — notify is unidirectional / fire-and-forget.)

──────────────────────────────────────────────
Notes
──────────────────────────────────────────────
- The lower agent_id always initiates the QUIC connection (initiator rule).
- All messages use 4-byte big-endian length-prefix framing over QUIC streams.
- Bidirectional streams are used for request-response (hello, ping, query, delegate, cancel, discover).
- Unidirectional streams are used for fire-and-forget (notify, result).
- The hello handshake must complete before any other messages on a connection.
"#);
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn send_ipc(command: Value, wait_for_correlated_inbound: bool) -> Result<String> {
    let paths = AxonPaths::discover()?;
    let mut stream = UnixStream::connect(&paths.socket)
        .await
        .with_context(|| {
            format!(
                "failed to connect to daemon socket: {}. Is the daemon running?",
                paths.socket.display()
            )
        })?;

    let line = serde_json::to_string(&command).context("failed to serialize IPC command")?;
    stream
        .write_all(line.as_bytes())
        .await
        .context("failed to write IPC command")?;
    stream
        .write_all(b"\n")
        .await
        .context("failed to write IPC newline")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    let bytes = reader
        .read_line(&mut response)
        .await
        .context("failed to read IPC response")?;
    if bytes == 0 {
        return Err(anyhow::anyhow!(
            "daemon closed connection without a response"
        ));
    }

    let first: Value =
        serde_json::from_str(response.trim()).context("failed to decode IPC response")?;

    if !wait_for_correlated_inbound {
        return serde_json::to_string_pretty(&first).context("failed to encode response");
    }

    let Some(msg_id) = first.get("msg_id").and_then(Value::as_str) else {
        return serde_json::to_string_pretty(&first).context("failed to encode response");
    };
    if first.get("ok") != Some(&json!(true)) {
        return serde_json::to_string_pretty(&first).context("failed to encode response");
    }
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return serde_json::to_string_pretty(&first).context("timed out waiting for response");
        }

        let mut line = String::new();
        let bytes = timeout(remaining, reader.read_line(&mut line))
            .await
            .context("timed out waiting for correlated response")?
            .context("failed reading correlated response")?;
        if bytes == 0 {
            return serde_json::to_string_pretty(&first).context("connection closed");
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: Value =
            serde_json::from_str(trimmed).context("failed decoding inbound IPC line")?;
        let is_match = value.get("inbound") == Some(&json!(true))
            && value
                .get("envelope")
                .and_then(|e| e.get("ref"))
                .and_then(Value::as_str)
                == Some(msg_id);
        if is_match {
            return serde_json::to_string_pretty(&value).context("failed encoding response");
        }
    }
}
