use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
#[cfg(feature = "generate-docs")]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use serde_json::json;
use tracing_subscriber::EnvFilter;

use axon::config::{
    AxonPaths, Config, PeerAddr, PersistedStaticPeerConfig, load_persisted_config,
    save_persisted_config,
};
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;
use axon::peer_token;

mod cli;
mod cli_examples;
mod doctor;

#[derive(Debug, Parser)]
#[command(
    name = "axon",
    about = "AXON — Agent eXchange Over Network",
    version = env!("CARGO_PKG_VERSION"),
    propagate_version = true
)]
struct Cli {
    /// AXON state root directory (socket/identity/config/known_peers).
    /// Falls back to AXON_ROOT, then ~/.axon.
    #[arg(
        long = "state-root",
        visible_aliases = ["state", "root"],
        global = true,
        value_name = "DIR"
    )]
    state_root: Option<PathBuf>,

    /// Enable debug-level logging when RUST_LOG is unset.
    #[arg(long, short = 'v', global = true)]
    verbose: bool,

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
    },
    /// Send a request to another agent and wait for a response.
    Request {
        #[arg(value_parser = parse_agent_id_arg)]
        agent_id: String,
        /// Timeout in seconds while waiting for a response.
        #[arg(
            long,
            value_name = "SECONDS",
            default_value_t = 30,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        timeout: u64,
        /// Text payload (sent as {"message":"<TEXT>"} on the wire).
        message: String,
    },
    /// Send a fire-and-forget message to another agent.
    Notify {
        #[arg(value_parser = parse_agent_id_arg)]
        agent_id: String,
        /// Parse payload as JSON (default sends literal text). Payload is sent as {"data": <value>}.
        #[arg(long)]
        json: bool,
        /// Payload data (sent as {"data":"<TEXT>"}, or {"data":<JSON>} with --json).
        data: String,
    },
    /// List discovered and connected peers.
    Peers {
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show daemon status.
    Status {
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print this agent's identity.
    Identity {
        /// Print rich identity metadata as JSON.
        #[arg(long)]
        json: bool,
        /// Override address used for URI output (`host:port` or `ip:port`).
        #[arg(long, value_name = "ADDR")]
        addr: Option<String>,
    },
    /// Enroll a peer from an `axon://` token.
    Connect { token: String },
    /// Print running daemon identity and metadata via IPC.
    Whoami {
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Diagnose local AXON state and optionally apply safe repairs.
    Doctor(doctor::DoctorArgs),
    /// Read/write scalar config values.
    Config(cli::config_cmd::ConfigArgs),
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
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let Cli {
        state_root,
        verbose: _,
        command,
    } = cli;
    let resolve_paths = || AxonPaths::discover_with_override(state_root.as_deref());

    match command {
        Commands::Daemon { port, disable_mdns } => {
            let paths = resolve_paths()?;
            run_daemon(DaemonOptions {
                port,
                disable_mdns,
                axon_root: Some(paths.root.clone()),
                cancel: None,
            })
            .await?;
        }
        Commands::Request {
            agent_id,
            timeout,
            message,
        } => {
            let paths = resolve_paths()?;
            let payload = json!({ "message": message });
            let response = cli::ipc_client::send_ipc(
                &paths,
                json!({
                    "cmd": "send",
                    "to": agent_id,
                    "kind": "request",
                    "timeout_secs": timeout,
                    "payload": payload
                }),
            )
            .await?;
            print_json_value(&response)?;
            return Ok(cli::ipc_client::daemon_reply_exit_code(
                &response,
                cli::ipc_client::ResponseMode::Request,
            ));
        }
        Commands::Notify {
            agent_id,
            json,
            data,
        } => {
            let paths = resolve_paths()?;
            let parsed_data = cli::notify_payload::parse_notify_payload(&data, json)?;
            let payload = json!({ "data": parsed_data });
            let response = cli::ipc_client::send_ipc(
                &paths,
                json!({"cmd": "send", "to": agent_id, "kind": "message", "payload": payload}),
            )
            .await?;
            print_json_value(&response)?;
            return Ok(cli::ipc_client::daemon_reply_exit_code(
                &response,
                cli::ipc_client::ResponseMode::Generic,
            ));
        }
        Commands::Peers { json } => {
            let paths = resolve_paths()?;
            let response = cli::ipc_client::send_ipc(&paths, json!({"cmd": "peers"})).await?;
            if json {
                print_json_value(&response)?;
            } else if let Some(rendered) = cli::format::render_peers_human(&response) {
                println!("{rendered}");
            } else {
                print_json_value(&response)?;
            }
            return Ok(cli::ipc_client::daemon_reply_exit_code(
                &response,
                cli::ipc_client::ResponseMode::Generic,
            ));
        }
        Commands::Status { json } => {
            let paths = resolve_paths()?;
            let response = cli::ipc_client::send_ipc(&paths, json!({"cmd": "status"})).await?;
            if json {
                print_json_value(&response)?;
            } else if let Some(rendered) = cli::format::render_status_human(&response) {
                println!("{rendered}");
            } else {
                print_json_value(&response)?;
            }
            return Ok(cli::ipc_client::daemon_reply_exit_code(
                &response,
                cli::ipc_client::ResponseMode::Generic,
            ));
        }
        Commands::Identity { json, addr } => {
            let paths = resolve_paths()?;
            let identity = Identity::load_or_generate(&paths)?;
            let config = Config::load(&paths.config).await?;
            let port = config.effective_port(None);
            let addr =
                select_identity_addr(addr.as_deref(), config.advertise_addr.as_deref(), port)
                    .context("failed to determine identity advertise address")?;
            let uri = peer_token::encode(identity.public_key_base64(), &addr)
                .context("failed to construct peer URI")?;
            if json {
                let (addr_host, addr_port) = split_addr_port(&addr)?;
                let rendered = cli::identity_output::render_identity_json(
                    identity.agent_id(),
                    identity.public_key_base64(),
                    &addr_host,
                    addr_port,
                    &uri,
                )?;
                println!("{rendered}");
            } else {
                println!("{}", cli::identity_output::render_identity_human(&uri));
            }
        }
        Commands::Connect { token } => {
            let paths = resolve_paths()?;
            let identity = Identity::load_or_generate(&paths)?;
            let decoded = peer_token::decode(&token).context("failed to parse peer token")?;

            if decoded.agent_id.as_str() == identity.agent_id() {
                anyhow::bail!("refusing to enroll self ({})", decoded.agent_id);
            }

            let mut persisted = load_persisted_config(&paths.config).await?;
            if persisted
                .peers
                .iter()
                .any(|peer| peer.agent_id.as_str() == decoded.agent_id.as_str())
            {
                anyhow::bail!(
                    "peer {} already exists in {}",
                    decoded.agent_id,
                    paths.config.display()
                );
            }

            let parsed_addr =
                PeerAddr::parse(&decoded.addr).context("peer token has invalid addr")?;
            persisted.peers.push(PersistedStaticPeerConfig {
                agent_id: decoded.agent_id.clone(),
                addr: parsed_addr,
                pubkey: decoded.pubkey.clone(),
            });
            save_persisted_config(&paths.config, &persisted).await?;

            if paths.socket.exists() {
                let hotload = cli::ipc_client::send_ipc(
                    &paths,
                    json!({"cmd": "add_peer", "pubkey": decoded.pubkey, "addr": decoded.addr}),
                )
                .await;
                match hotload {
                    Ok(response) if response.get("ok") == Some(&json!(true)) => {}
                    Ok(response) => {
                        let rendered = serde_json::to_string_pretty(&response)
                            .unwrap_or_else(|_| response.to_string());
                        anyhow::bail!(
                            "peer saved to {} but daemon hot-load failed.\nDaemon response: {}",
                            paths.config.display(),
                            rendered
                        );
                    }
                    Err(err) => {
                        anyhow::bail!(
                            "peer saved to {} but daemon hot-load failed: {}",
                            paths.config.display(),
                            err
                        );
                    }
                }
            }

            println!("✓ Added peer {} ({})", decoded.agent_id, decoded.addr);
        }
        Commands::Whoami { json } => {
            let paths = resolve_paths()?;
            let response = cli::ipc_client::send_ipc(&paths, json!({"cmd": "whoami"})).await?;
            if json {
                print_json_value(&response)?;
            } else if let Some(rendered) = cli::format::render_whoami_human(&response) {
                println!("{rendered}");
            } else {
                print_json_value(&response)?;
            }
            return Ok(cli::ipc_client::daemon_reply_exit_code(
                &response,
                cli::ipc_client::ResponseMode::Generic,
            ));
        }
        Commands::Doctor(args) => {
            let paths = resolve_paths()?;
            let report = doctor::run(&paths, &args).await?;
            if args.json {
                let rendered = serde_json::to_string_pretty(&report)
                    .context("failed to encode doctor output")?;
                println!("{rendered}");
            } else {
                println!("{}", cli::format::render_doctor_human(&report));
            }
            if report.ok {
                return Ok(ExitCode::SUCCESS);
            }
            return Ok(ExitCode::from(2));
        }
        Commands::Config(args) => {
            let paths = resolve_paths()?;
            return cli::config_cmd::run(&paths, args).await;
        }
        Commands::Examples => {
            cli_examples::print_annotated_examples();
        }
        #[cfg(feature = "generate-docs")]
        Commands::GenDocs { out_dir } => {
            generate_docs(&out_dir)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn print_json_value(value: &serde_json::Value) -> Result<()> {
    println!("{}", cli::ipc_client::render_json(value)?);
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

fn init_tracing(verbose: bool) {
    let default = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn parse_agent_id_arg(input: &str) -> std::result::Result<String, String> {
    canonicalize_agent_id(input)
        .ok_or_else(|| format!("invalid agent_id '{input}'; expected format ed25519.<32 hex>"))
}

fn canonicalize_agent_id(input: &str) -> Option<String> {
    let (prefix, hex) = input.split_once('.')?;
    if !prefix.eq_ignore_ascii_case("ed25519") {
        return None;
    }
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("ed25519.{}", hex.to_ascii_lowercase()))
}

fn select_identity_addr(
    addr_override: Option<&str>,
    advertise_addr: Option<&str>,
    port: u16,
) -> Result<String> {
    if let Some(addr) = addr_override {
        return normalize_addr(addr);
    }
    if let Some(addr) = advertise_addr {
        return normalize_addr(addr);
    }

    let ip = discover_default_route_ip().context(
        "unable to auto-discover a routable IP; pass --addr host:port or set advertise_addr in config.yaml",
    )?;
    Ok(format!("{ip}:{port}"))
}

fn normalize_addr(input: &str) -> Result<String> {
    let parsed = PeerAddr::parse(input)?;
    Ok(parsed.to_string())
}

fn split_addr_port(addr: &str) -> Result<(String, u16)> {
    let parsed = PeerAddr::parse(addr)?;
    match parsed {
        PeerAddr::Socket(socket) => Ok((socket.ip().to_string(), socket.port())),
        PeerAddr::Host { host, port } => Ok((host, port)),
    }
}

fn discover_default_route_ip() -> Result<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")
        .context("failed to open UDP socket for route probe")?;
    socket
        .connect("8.8.8.8:80")
        .context("failed to perform UDP route probe")?;
    let local = socket
        .local_addr()
        .context("failed to read local address from route probe")?;
    Ok(local.ip())
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
