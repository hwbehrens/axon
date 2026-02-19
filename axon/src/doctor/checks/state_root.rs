use std::fs;
use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result};

use axon::config::AxonPaths;

use crate::doctor::{DoctorArgs, DoctorReport};

pub(in crate::doctor) fn check_state_root(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if !paths.root.exists() {
        if args.fix {
            paths.ensure_root_exists()?;
            report.add_fix(
                "state_root_create",
                format!("created {}", paths.root.display()),
            );
            report.add_check(
                "state_root",
                true,
                true,
                format!("state root created at {}", paths.root.display()),
            );
        } else {
            report.add_check(
                "state_root",
                false,
                true,
                format!(
                    "state root does not exist: {} (run `axon doctor --fix` to create)",
                    paths.root.display()
                ),
            );
        }
        return Ok(());
    }

    let meta = fs::symlink_metadata(&paths.root)
        .with_context(|| format!("failed to read metadata: {}", paths.root.display()))?;
    if meta.file_type().is_symlink() {
        report.add_check(
            "state_root",
            false,
            false,
            format!(
                "state root is a symlink: {} (security violation; remove symlink manually)",
                paths.root.display()
            ),
        );
        return Ok(());
    }

    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        if args.fix {
            fs::set_permissions(&paths.root, fs::Permissions::from_mode(0o700)).with_context(
                || {
                    format!(
                        "failed to set state root permissions: {}",
                        paths.root.display()
                    )
                },
            )?;
            report.add_fix(
                "state_root_permissions",
                format!("set {} to 700", paths.root.display()),
            );
            report.add_check(
                "state_root",
                true,
                true,
                format!(
                    "state root permissions normalized to 700 ({})",
                    paths.root.display()
                ),
            );
        } else {
            report.add_check(
                "state_root",
                false,
                true,
                format!(
                    "state root permissions are {:o}, expected 700 ({})",
                    mode,
                    paths.root.display()
                ),
            );
        }
    } else {
        report.add_check(
            "state_root",
            true,
            false,
            format!("state root looks healthy ({})", paths.root.display()),
        );
    }

    Ok(())
}
