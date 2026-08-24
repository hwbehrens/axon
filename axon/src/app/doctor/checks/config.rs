use anyhow::Result;

use axon::config::{AxonPaths, Config, PersistedConfig, save_persisted_config};

use crate::app::doctor::{DoctorArgs, DoctorReport};

use super::backup_file_with_timestamp;

pub(in crate::app::doctor) async fn check_config(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if !paths.config.exists() {
        report.add_check("config", true, false, "config.yaml not present".to_string());
        return Ok(());
    }

    match Config::load(&paths.config).await {
        Ok(cfg) => {
            report.add_check(
                "config",
                true,
                false,
                format!(
                    "config.yaml parsed (name: {}, port: {}, advertise_addr: {})",
                    cfg.name.as_deref().unwrap_or("unset"),
                    cfg.port
                        .map_or_else(|| "default".to_string(), |port| port.to_string()),
                    cfg.advertise_addr.as_deref().unwrap_or("unset")
                ),
            );
        }
        Err(err) => {
            if args.fix {
                let backup = backup_file_with_timestamp(&paths.config)?;
                save_persisted_config(&paths.config, &PersistedConfig::default()).await?;
                report.add_fix(
                    "config_reset",
                    format!(
                        "backed up unsupported or corrupt config.yaml to {} and reset local settings to defaults",
                        backup.display()
                    ),
                );
                report.add_check(
                    "config",
                    true,
                    true,
                    "corrupt config.yaml reset to defaults".to_string(),
                );
            } else {
                report.add_check(
                    "config",
                    false,
                    true,
                    format!(
                        "config.yaml parse/load error: {err}; run `axon doctor --fix` to back up and reset"
                    ),
                );
            }
        }
    }

    Ok(())
}
