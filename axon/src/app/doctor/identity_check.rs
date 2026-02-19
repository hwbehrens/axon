use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use axon::config::AxonPaths;
use axon::identity::Identity;

use super::checks::backup_file_with_timestamp;
use super::{DoctorArgs, DoctorReport};

pub(super) fn check_identity(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    match inspect_identity_key(&paths.identity_key)? {
        IdentityHealth::Valid => {
            report.add_check(
                "identity",
                true,
                false,
                "identity.key is valid base64 seed".to_string(),
            );
        }
        IdentityHealth::Missing => {
            if args.fix {
                let identity = Identity::load_or_generate(paths)?;
                report.add_fix(
                    "identity_generate",
                    format!("generated new identity ({})", identity.agent_id()),
                );
                report.add_check(
                    "identity",
                    true,
                    true,
                    format!("generated new identity ({})", identity.agent_id()),
                );
            } else {
                report.add_check(
                    "identity",
                    false,
                    true,
                    "identity.key missing (run `axon doctor --fix` to generate)".to_string(),
                );
            }
        }
        IdentityHealth::Invalid(reason) => {
            if args.fix && args.rekey {
                let backups = backup_identity_files(paths)?;

                if paths.identity_key.exists() {
                    fs::remove_file(&paths.identity_key).with_context(|| {
                        format!(
                            "failed removing unrecoverable identity key: {}",
                            paths.identity_key.display()
                        )
                    })?;
                }

                if paths.identity_pub.exists() {
                    fs::remove_file(&paths.identity_pub).with_context(|| {
                        format!(
                            "failed removing stale public key: {}",
                            paths.identity_pub.display()
                        )
                    })?;
                }

                let identity = Identity::load_or_generate(paths)?;
                let backup_summary = if backups.is_empty() {
                    "no backup files created".to_string()
                } else {
                    format!("backup files: {}", backups.join(", "))
                };
                report.add_fix(
                    "identity_rekey",
                    format!(
                        "rekeyed identity after unrecoverable key ({backup_summary}) ({})",
                        identity.agent_id()
                    ),
                );
                report.add_check(
                    "identity",
                    true,
                    true,
                    format!(
                        "unrecoverable key replaced with fresh identity ({})",
                        identity.agent_id()
                    ),
                );
            } else {
                report.add_check(
                    "identity",
                    false,
                    true,
                    format!(
                        "unrecoverable identity.key: {reason}; run `axon doctor --fix --rekey` to back up and regenerate identity"
                    ),
                );
            }
        }
    }

    Ok(())
}

fn backup_identity_files(paths: &AxonPaths) -> Result<Vec<String>> {
    let mut backups = Vec::new();

    if paths.identity_key.exists() {
        let backup = backup_file_with_timestamp(&paths.identity_key)?;
        backups.push(backup.display().to_string());
    }

    if paths.identity_pub.exists() {
        let backup = backup_file_with_timestamp(&paths.identity_pub)?;
        backups.push(backup.display().to_string());
    }

    Ok(backups)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentityHealth {
    Valid,
    Missing,
    Invalid(String),
}

fn inspect_identity_key(path: &Path) -> Result<IdentityHealth> {
    if !path.exists() {
        return Ok(IdentityHealth::Missing);
    }

    let raw = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    match std::str::from_utf8(&raw) {
        Ok(text) => {
            if decode_seed_text_for_check(text).is_ok() {
                Ok(IdentityHealth::Valid)
            } else {
                Ok(IdentityHealth::Invalid(
                    "base64 decode failed for identity.key".to_string(),
                ))
            }
        }
        Err(_) => Ok(IdentityHealth::Invalid(
            "identity.key is non-UTF-8; expected base64 text".to_string(),
        )),
    }
}

fn decode_seed_text_for_check(text: &str) -> Result<[u8; 32]> {
    let bytes = STANDARD
        .decode(text.trim())
        .context("identity.key base64 decode failed")?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("decoded identity key length {} != 32", v.len()))?;
    Ok(seed)
}
