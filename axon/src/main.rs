use anyhow::Result;
use base64::Engine;
use axon::daemon::Daemon;
use axon::ipc::{IpcCommand, IpcResponse};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::net::UnixStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Daemon {
        #[arg(short, long)]
        port: Option<u16>,
    },
    Send {
        agent_id: String,
        message: String,
    },
    Peers,
    Status,
    Identity,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let home_dir = dirs::home_dir().expect("Could not find home directory");

    match cli.command {
        Commands::Daemon { port } => {
            let daemon = Daemon::new(home_dir, port).await?;
            daemon.run().await?;
        }
        Commands::Peers => {
            let response = send_command(home_dir, IpcCommand::Peers).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Commands::Status => {
            let response = send_command(home_dir, IpcCommand::Status).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        Commands::Identity => {
            let identity = axon::identity::Identity::load_or_generate(&home_dir).await?;
            println!("Agent ID: {}", identity.agent_id());
            println!("Public Key: {}", base64::engine::general_purpose::STANDARD.encode(identity.verifying_key().to_bytes()));
        }
        Commands::Send { agent_id, message } => {
            let response = send_command(home_dir, IpcCommand::Send {
                to: agent_id,
                kind: "query".to_string(),
                payload: serde_json::json!({ "question": message }),
            }).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
    }

    Ok(())
}

async fn send_command(home_dir: PathBuf, cmd: IpcCommand) -> Result<IpcResponse> {
    let socket_path = home_dir.join(".axon/axon.sock");
    let mut stream = UnixStream::connect(socket_path).await?;
    
    let cmd_json = serde_json::to_string(&cmd)? + "\n";
    stream.write_all(cmd_json.as_bytes()).await?;
    
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    
    let response: IpcResponse = serde_json::from_str(&line)?;
    Ok(response)
}
