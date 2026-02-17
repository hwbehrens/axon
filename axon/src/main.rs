use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
#[cfg(feature = "generate-docs")]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing_subscriber::EnvFilter;

use axon::config::AxonPaths;
use axon::daemon::{DaemonOptions, run_daemon};
use axon::identity::Identity;

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
    Send {
        #[arg(value_parser = parse_agent_id_arg)]
        agent_id: String,
        message: String,
    },
    /// Send a fire-and-forget message to another agent.
    Notify {
        #[arg(value_parser = parse_agent_id_arg)]
        agent_id: String,
        /// Force data to be treated as raw text payload.
        #[arg(long)]
        text: bool,
        data: String,
    },
    /// List discovered and connected peers.
    Peers,
    /// Show daemon status.
    Status,
    /// Print this agent's identity.
    Identity {
        /// Print a ready-to-copy static peer TOML snippet for this identity.
        #[arg(long)]
        peer_config: bool,
    },
    /// Print running daemon identity and metadata via IPC.
    Whoami,
    /// Diagnose local AXON state and optionally apply safe repairs.
    Doctor(doctor::DoctorArgs),
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
        Commands::Send { agent_id, message } => {
            let paths = resolve_paths()?;
            let payload = json!({ "message": message });
            let response = send_ipc(
                &paths,
                json!({"cmd": "send", "to": agent_id, "kind": "request", "payload": payload}),
            )
            .await?;
            return print_ipc_response_and_classify(response);
        }
        Commands::Notify {
            agent_id,
            text,
            data,
        } => {
            let paths = resolve_paths()?;
            let parsed_data = parse_notify_payload(&data, text)?;
            let payload = json!({ "data": parsed_data });
            let response = send_ipc(
                &paths,
                json!({"cmd": "send", "to": agent_id, "kind": "message", "payload": payload}),
            )
            .await?;
            return print_ipc_response_and_classify(response);
        }
        Commands::Peers => {
            let paths = resolve_paths()?;
            let response = send_ipc(&paths, json!({"cmd": "peers"})).await?;
            return print_ipc_response_and_classify(response);
        }
        Commands::Status => {
            let paths = resolve_paths()?;
            let response = send_ipc(&paths, json!({"cmd": "status"})).await?;
            return print_ipc_response_and_classify(response);
        }
        Commands::Identity { peer_config } => {
            let paths = resolve_paths()?;
            let identity = Identity::load_or_generate(&paths)?;
            if peer_config {
                println!(
                    "{}",
                    peer_config_snippet(identity.agent_id(), identity.public_key_base64())
                );
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "agent_id": identity.agent_id(),
                        "public_key": identity.public_key_base64(),
                    }))
                    .context("failed to encode identity output")?
                );
            }
        }
        Commands::Whoami => {
            let paths = resolve_paths()?;
            let response = send_ipc(&paths, json!({"cmd": "whoami"})).await?;
            return print_ipc_response_and_classify(response);
        }
        Commands::Doctor(args) => {
            let paths = resolve_paths()?;
            let report = doctor::run(&paths, &args).await?;
            let rendered =
                serde_json::to_string_pretty(&report).context("failed to encode doctor output")?;
            println!("{rendered}");
            if report.ok {
                return Ok(ExitCode::SUCCESS);
            }
            return Ok(ExitCode::from(2));
        }
        #[cfg(feature = "generate-docs")]
        Commands::GenDocs { out_dir } => {
            generate_docs(&out_dir)?;
        }
        Commands::Examples => {
            cli_examples::print_annotated_examples();
        }
    }

    Ok(ExitCode::SUCCESS)
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

fn parse_notify_payload(data: &str, force_text: bool) -> Result<Value> {
    if force_text {
        return Ok(json!(data));
    }

    let trimmed = data.trim_start();
    let looks_json_like = matches!(trimmed.chars().next(), Some('{') | Some('[') | Some('"'));

    match serde_json::from_str::<Value>(data) {
        Ok(value) => Ok(value),
        Err(err) if looks_json_like => Err(anyhow!(
            "notify payload appears JSON-like but is invalid: {err}. \
             Fix the JSON or pass --text to send literal text."
        )),
        Err(_) => Ok(json!(data)),
    }
}

fn peer_config_snippet(agent_id: &str, public_key: &str) -> String {
    format!(
        "[[peers]]\nagent_id = \"{agent_id}\"\npubkey = \"{public_key}\"\naddr = \"<this-machine>:7100\""
    )
}

fn daemon_reply_exit_code(response: &Value) -> ExitCode {
    if response.get("ok") == Some(&json!(false)) {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn print_ipc_response_and_classify(response: Value) -> Result<ExitCode> {
    let rendered =
        serde_json::to_string_pretty(&response).context("failed to encode response output")?;
    println!("{rendered}");
    Ok(daemon_reply_exit_code(&response))
}

async fn send_ipc(paths: &AxonPaths, command: Value) -> Result<Value> {
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
    loop {
        let mut response = Vec::new();
        let bytes = reader
            .read_until(b'\n', &mut response)
            .await
            .context("failed to read IPC response")?;
        if bytes == 0 {
            return Err(anyhow::anyhow!(
                "daemon closed connection without a command response"
            ));
        }

        if response.last() == Some(&b'\n') {
            response.pop();
        }
        let line = std::str::from_utf8(&response)
            .context("failed to decode IPC response as UTF-8")?
            .trim();
        if line.is_empty() {
            continue;
        }

        let decoded: Value = serde_json::from_str(line).context("failed to decode IPC response")?;
        if decoded.get("event").and_then(Value::as_str) == Some("inbound") {
            continue;
        }
        return Ok(decoded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn daemon_reply_failure_maps_to_exit_2() {
        let code = daemon_reply_exit_code(&json!({"ok": false}));
        assert_eq!(code, ExitCode::from(2));
    }

    #[test]
    fn daemon_reply_success_maps_to_exit_0() {
        let code = daemon_reply_exit_code(&json!({"ok": true}));
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn parse_agent_id_arg_normalizes_case() {
        let parsed = parse_agent_id_arg("ED25519.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .expect("mixed/upper case should parse");
        assert_eq!(parsed, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn cli_verbose_flag_parses_short_and_long() {
        let long = Cli::try_parse_from(["axon", "--verbose", "status"]).expect("parse --verbose");
        assert!(long.verbose);
        assert!(matches!(long.command, Commands::Status));

        let short = Cli::try_parse_from(["axon", "-v", "status"]).expect("parse -v");
        assert!(short.verbose);
        assert!(matches!(short.command, Commands::Status));

        let default = Cli::try_parse_from(["axon", "status"]).expect("parse without verbose");
        assert!(!default.verbose);
        assert!(matches!(default.command, Commands::Status));
    }

    #[test]
    fn doctor_rekey_requires_fix_flag() {
        let err = Cli::try_parse_from(["axon", "doctor", "--rekey"])
            .expect_err("--rekey without --fix should fail");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn peer_config_snippet_has_expected_toml_shape() {
        let snippet = peer_config_snippet("ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Zm9vYmFy");
        assert_eq!(
            snippet,
            "[[peers]]\nagent_id = \"ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\npubkey = \"Zm9vYmFy\"\naddr = \"<this-machine>:7100\""
        );
    }
}
