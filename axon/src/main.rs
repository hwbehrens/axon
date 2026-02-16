use std::time::Duration;

use anyhow::{Context, Result};
#[cfg(feature = "generate-docs")]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{Instant, timeout};
use tracing_subscriber::EnvFilter;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;

mod cli_examples;

#[cfg(feature = "generate-docs")]
use std::path::PathBuf;

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
        /// Disable mDNS discovery (use static peers only).
        #[arg(long)]
        disable_mdns: bool,
        /// Override the derived agent_id (for testing/aliasing).
        #[arg(long)]
        agent_id: Option<String>,
    },
    /// Send a request to another agent and wait for a response.
    Send { agent_id: String, message: String },
    /// Send a fire-and-forget message to another agent.
    Notify { agent_id: String, data: String },
    /// List discovered and connected peers.
    Peers,
    /// Show daemon status.
    Status,
    /// Print this agent's identity.
    Identity,
    /// Print example interactions.
    Examples,
    /// Generate shell completions and man page (internal, for packaging).
    #[cfg(feature = "generate-docs")]
    #[command(hide = true)]
    GenDocs {
        /// Output directory (creates completions/ and man/ inside it).
        #[arg(long, value_name = "DIR")]
        out_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon {
            port,
            disable_mdns,
            agent_id,
        } => {
            run_daemon(DaemonOptions {
                port,
                disable_mdns,
                axon_root: None,
                agent_id,
                cancel: None,
            })
            .await?;
        }
        Commands::Send { agent_id, message } => {
            let payload = json!({ "message": message });
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "request", "payload": payload}),
                false,
            )
            .await?;
            println!("{line}");
        }
        Commands::Notify { agent_id, data } => {
            let parsed_data = serde_json::from_str::<Value>(&data).unwrap_or_else(|_| json!(data));
            let payload = json!({ "data": parsed_data });
            let line = send_ipc(
                json!({"cmd": "send", "to": agent_id, "kind": "message", "payload": payload}),
                false,
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
        #[cfg(feature = "generate-docs")]
        Commands::GenDocs { out_dir } => {
            generate_docs(&out_dir)?;
        }
        Commands::Examples => {
            cli_examples::print_annotated_examples();
        }
    }

    Ok(())
}

#[cfg(feature = "generate-docs")]
fn generate_docs(out_dir: &std::path::Path) -> Result<()> {
    use clap_complete::{Shell, generate_to};
    use clap_mangen::Man;
    use std::fs;

    let completions_dir = out_dir.join("completions");
    let man_dir = out_dir.join("man");

    fs::create_dir_all(&completions_dir)
        .with_context(|| format!("failed to create {}", completions_dir.display()))?;
    fs::create_dir_all(&man_dir)
        .with_context(|| format!("failed to create {}", man_dir.display()))?;

    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();

    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        generate_to(shell, &mut cmd, &bin_name, &completions_dir)
            .with_context(|| format!("failed generating {shell:?} completion"))?;
    }

    let man = Man::new(Cli::command());
    let mut buffer: Vec<u8> = Vec::new();
    man.render(&mut buffer)
        .context("failed rendering man page")?;
    let man_path = man_dir.join("axon.1");
    fs::write(&man_path, &buffer)
        .with_context(|| format!("failed writing {}", man_path.display()))?;

    eprintln!(
        "Generated completions and man page in {}",
        out_dir.display()
    );
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn send_ipc(command: Value, wait_for_correlated_inbound: bool) -> Result<String> {
    let paths = AxonPaths::discover()?;
    let mut stream = UnixStream::connect(&paths.socket).await.with_context(|| {
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
        let is_match = value.get("event") == Some(&json!("inbound"))
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
