use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use axon::config::{
    AxonPaths, PeerAddr, PersistedConfig, load_persisted_config, save_persisted_config,
};
use clap::{ArgGroup, Args, ValueEnum};
use serde_json::{Map, Value, json};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConfigKey {
    Name,
    Port,
    AdvertiseAddr,
}

#[derive(Debug, Clone, Args)]
#[command(group(
    ArgGroup::new("mode")
        .args(["list", "unset", "edit"])
        .multiple(false)
))]
pub struct ConfigArgs {
    /// List configured scalar values.
    #[arg(long)]
    pub list: bool,
    /// Unset a key (set it back to None).
    #[arg(long, value_enum, value_name = "KEY")]
    pub unset: Option<ConfigKey>,
    /// Open config.yaml in $EDITOR.
    #[arg(long)]
    pub edit: bool,
    /// JSON output (supported with --list).
    #[arg(long)]
    pub json: bool,
    /// Config key (get/set mode).
    #[arg(value_enum)]
    pub key: Option<ConfigKey>,
    /// Value (set mode).
    pub value: Option<String>,
}

#[derive(Debug)]
enum ConfigAction {
    List,
    Unset(ConfigKey),
    Edit,
    Get(ConfigKey),
    Set(ConfigKey, String),
}

pub async fn run(paths: &AxonPaths, args: ConfigArgs) -> Result<ExitCode> {
    let action = parse_action(&args)?;

    match action {
        ConfigAction::List => {
            let persisted = load_persisted_config(&paths.config).await?;
            if args.json {
                println!("{}", render_list_json(&persisted)?);
            } else {
                let rendered = render_list_text(&persisted);
                if !rendered.is_empty() {
                    println!("{rendered}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ConfigAction::Unset(key) => {
            let mut persisted = load_persisted_config(&paths.config).await?;
            unset_value(&mut persisted, key);
            save_persisted_config(&paths.config, &persisted).await?;
            Ok(ExitCode::SUCCESS)
        }
        ConfigAction::Edit => {
            ensure_config_file(paths).await?;
            open_in_editor(&paths.config).await?;
            Ok(ExitCode::SUCCESS)
        }
        ConfigAction::Get(key) => {
            let persisted = load_persisted_config(&paths.config).await?;
            if let Some(value) = get_value(&persisted, key) {
                println!("{value}");
                Ok(ExitCode::SUCCESS)
            } else {
                eprintln!("{}: not set", key_display_name(key));
                Ok(ExitCode::from(1))
            }
        }
        ConfigAction::Set(key, value) => {
            let mut persisted = load_persisted_config(&paths.config).await?;
            apply_set(&mut persisted, key, &value)?;
            save_persisted_config(&paths.config, &persisted).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn parse_action(args: &ConfigArgs) -> Result<ConfigAction> {
    if args.json && !args.list {
        anyhow::bail!("--json is only supported with --list");
    }

    if args.list {
        if args.unset.is_some() || args.edit || args.key.is_some() || args.value.is_some() {
            anyhow::bail!("--list cannot be combined with positional args or other modes");
        }
        return Ok(ConfigAction::List);
    }

    if let Some(key) = args.unset {
        if args.edit || args.key.is_some() || args.value.is_some() {
            anyhow::bail!("--unset cannot be combined with positional args or --edit");
        }
        return Ok(ConfigAction::Unset(key));
    }

    if args.edit {
        if args.key.is_some() || args.value.is_some() {
            anyhow::bail!("--edit cannot be combined with positional args");
        }
        return Ok(ConfigAction::Edit);
    }

    let Some(key) = args.key else {
        anyhow::bail!(
            "missing config key. Use `axon config --list`, `axon config --unset <KEY>`, `axon config --edit`, or `axon config <KEY> [VALUE]`"
        );
    };

    if let Some(value) = args.value.clone() {
        return Ok(ConfigAction::Set(key, value));
    }

    Ok(ConfigAction::Get(key))
}

fn key_display_name(key: ConfigKey) -> &'static str {
    match key {
        ConfigKey::Name => "name",
        ConfigKey::Port => "port",
        ConfigKey::AdvertiseAddr => "advertise-addr",
    }
}

fn get_value(config: &PersistedConfig, key: ConfigKey) -> Option<String> {
    match key {
        ConfigKey::Name => config.name.clone(),
        ConfigKey::Port => config.port.map(|value| value.to_string()),
        ConfigKey::AdvertiseAddr => config.advertise_addr.clone(),
    }
}

fn apply_set(config: &mut PersistedConfig, key: ConfigKey, value: &str) -> Result<()> {
    match key {
        ConfigKey::Name => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                anyhow::bail!("name cannot be empty");
            }
            config.name = Some(trimmed.to_string());
        }
        ConfigKey::Port => {
            let port = value
                .parse::<u16>()
                .with_context(|| format!("invalid port '{value}'"))?;
            if port == 0 {
                anyhow::bail!(
                    "port 0 is not valid; QUIC requires a non-zero port for peer connections"
                );
            }
            config.port = Some(port);
        }
        ConfigKey::AdvertiseAddr => {
            let parsed = PeerAddr::parse(value)
                .with_context(|| format!("invalid advertise_addr '{value}'"))?;
            config.advertise_addr = Some(parsed.to_string());
        }
    }
    Ok(())
}

fn unset_value(config: &mut PersistedConfig, key: ConfigKey) {
    match key {
        ConfigKey::Name => config.name = None,
        ConfigKey::Port => config.port = None,
        ConfigKey::AdvertiseAddr => config.advertise_addr = None,
    }
}

fn render_list_text(config: &PersistedConfig) -> String {
    let mut lines = Vec::new();
    if let Some(name) = &config.name {
        lines.push(format!("name={name}"));
    }
    if let Some(port) = config.port {
        lines.push(format!("port={port}"));
    }
    if let Some(addr) = &config.advertise_addr {
        lines.push(format!("advertise_addr={addr}"));
    }
    lines.join("\n")
}

fn render_list_json(config: &PersistedConfig) -> Result<String> {
    let mut map = Map::new();
    if let Some(name) = &config.name {
        map.insert("name".to_string(), json!(name));
    }
    if let Some(port) = config.port {
        map.insert("port".to_string(), json!(port));
    }
    if let Some(addr) = &config.advertise_addr {
        map.insert("advertise_addr".to_string(), json!(addr));
    }

    serde_json::to_string_pretty(&Value::Object(map)).context("failed to encode config list JSON")
}

async fn ensure_config_file(paths: &AxonPaths) -> Result<()> {
    if let Some(parent) = paths.config.parent()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    if !paths.config.exists() {
        save_persisted_config(&paths.config, &PersistedConfig::default()).await?;
    }

    Ok(())
}

async fn open_in_editor(config_path: &std::path::Path) -> Result<()> {
    let editor = std::env::var("EDITOR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("$EDITOR is not set; cannot open config file"))?;

    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| anyhow!("$EDITOR is empty; cannot open config file"))?;

    let mut command = Command::new(program);
    command.args(parts);
    command.arg(config_path);

    let status = command.status().await.context("failed to run $EDITOR")?;
    if !status.success() {
        anyhow::bail!("editor exited with status {status}");
    }

    Ok(())
}

#[cfg(test)]
#[path = "config_cmd_tests.rs"]
mod tests;
