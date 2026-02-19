use anyhow::Result;

use axon::config::{AxonPaths, Config, PersistedConfig, save_persisted_config};

use crate::doctor::{DoctorArgs, DoctorReport};

use super::backup_file_with_timestamp;

pub(in crate::doctor) async fn check_config(
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
                format!("config.yaml parsed ({} static peers)", cfg.peers.len()),
            );
        }
        Err(err) => {
            if args.fix {
                let backup = backup_file_with_timestamp(&paths.config)?;
                save_persisted_config(&paths.config, &PersistedConfig::default()).await?;
                report.add_fix(
                    "config_reset",
                    format!(
                        "backed up corrupt config.yaml to {} and reset to defaults (peer enrollments lost — re-run `axon connect` to restore)",
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
