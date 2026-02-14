mod crypto;
mod discovery;
mod protocol;
mod socket;
mod transport;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(name = "acp", version = "0.1.0", about = "Agent Communication Protocol")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the ACP daemon
    Daemon {
        #[arg(long, default_value = "7100")]
        port: u16,
        #[arg(long)]
        agent_id: Option<String>,
    },
    /// Send a query message to a peer
    Send {
        /// Target agent ID
        agent_id: String,
        /// Message text
        message: String,
    },
    /// Delegate a task to a peer
    Delegate {
        /// Target agent ID
        agent_id: String,
        /// Task description
        task: String,
    },
    /// Broadcast a notification
    Notify {
        /// Topic
        topic: String,
        /// Data
        data: String,
    },
    /// List discovered peers
    Peers,
    /// Daemon status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "acp=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon { port, agent_id } => run_daemon(port, agent_id).await,
        Commands::Send { agent_id, message } => {
            send_ipc_command(&serde_json::json!({
                "cmd": "send",
                "to": agent_id,
                "message": message,
            }))
            .await
        }
        Commands::Delegate { agent_id, task } => {
            send_ipc_command(&serde_json::json!({
                "cmd": "send",
                "to": agent_id,
                "task": task,
            }))
            .await
        }
        Commands::Notify { topic, data } => {
            send_ipc_command(&serde_json::json!({
                "cmd": "send",
                "topic": topic,
                "data": data,
            }))
            .await
        }
        Commands::Peers => {
            send_ipc_command(&serde_json::json!({"cmd": "peers"})).await
        }
        Commands::Status => {
            send_ipc_command(&serde_json::json!({"cmd": "status"})).await
        }
    }
}

async fn run_daemon(port: u16, agent_id_opt: Option<String>) -> Result<()> {
    let identity = Arc::new(crypto::Identity::load_or_generate()?);
    let agent_id = agent_id_opt.unwrap_or_else(|| {
        std::env::var("ACP_AGENT_ID").unwrap_or_else(|_| "agent".to_string())
    });

    tracing::info!(%agent_id, pubkey = %identity.public_key_b64(), "Starting ACP daemon");

    let peers = discovery::new_peer_table();
    let connections = transport::new_connection_table();
    let (inbound_tx, inbound_rx) = transport::inbound_channel();

    // mDNS
    let mdns = mdns_sd::ServiceDaemon::new()
        .map_err(|e| anyhow::anyhow!("mDNS daemon: {e}"))?;
    discovery::advertise(&mdns, &agent_id, port, &identity.public_key_b64())?;
    discovery::browse(mdns.clone(), peers.clone(), agent_id.clone()).await?;

    // Peer cleanup
    tokio::spawn(discovery::cleanup_loop(peers.clone()));

    // TCP listener
    let tcp_identity = identity.clone();
    let tcp_peers = peers.clone();
    let tcp_inbound = inbound_tx.clone();
    let tcp_conns = connections.clone();
    tokio::spawn(async move {
        if let Err(e) =
            transport::listen(port, tcp_identity, tcp_peers, tcp_inbound, tcp_conns).await
        {
            tracing::error!("TCP listener error: {e}");
        }
    });

    // Proactive connections
    let conn_id = agent_id.clone();
    let conn_identity = identity.clone();
    let conn_peers = peers.clone();
    let conn_inbound = inbound_tx.clone();
    let conn_table = connections.clone();
    tokio::spawn(async move {
        transport::connect_to_peers(conn_id, conn_identity, conn_peers, conn_inbound, conn_table)
            .await;
    });

    // Unix socket IPC
    let state = Arc::new(socket::DaemonState {
        identity,
        agent_id: agent_id.clone(),
        peers,
        connections,
        start_time: Instant::now(),
        messages_sent: Arc::new(RwLock::new(0)),
    });

    socket::listen(state, inbound_rx).await
}

/// Connect to the daemon's Unix socket and send a command.
async fn send_ipc_command(cmd: &serde_json::Value) -> Result<()> {
    // Determine agent_id for socket path - try default
    let path = socket::socket_path("default");

    let stream = UnixStream::connect(&path).await.map_err(|e| {
        anyhow::anyhow!("Cannot connect to daemon at {path}: {e}\nIs the daemon running?")
    })?;

    let (reader, mut writer) = stream.into_split();
    let line = serde_json::to_string(cmd)? + "\n";
    writer.write_all(line.as_bytes()).await?;

    let mut lines = BufReader::new(reader).lines();
    if let Some(resp) = lines.next_line().await? {
        // Pretty print
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp) {
            println!("{}", serde_json::to_string_pretty(&val)?);
        } else {
            println!("{resp}");
        }
    }

    Ok(())
}
