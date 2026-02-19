use anyhow::Result;
use clap::Args;
use serde::Serialize;

use axon::config::AxonPaths;

mod checks;
mod identity_check;

#[derive(Debug, Clone, Args)]
pub struct DoctorArgs {
    /// Print machine-readable JSON report.
    #[arg(long)]
    pub json: bool,
    /// Apply safe local fixes for detected issues.
    #[arg(long)]
    pub fix: bool,
    /// Allow destructive identity reset when key data is unrecoverable.
    #[arg(long, requires = "fix")]
    pub rekey: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    pub fixable: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorFix {
    pub name: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub mode: &'static str,
    pub state_root: String,
    pub checks: Vec<DoctorCheck>,
    pub fixes_applied: Vec<DoctorFix>,
}

impl DoctorReport {
    fn new(paths: &AxonPaths, fix: bool) -> Self {
        Self {
            ok: true,
            mode: if fix { "fix" } else { "check" },
            state_root: paths.root.display().to_string(),
            checks: Vec::new(),
            fixes_applied: Vec::new(),
        }
    }

    fn add_check(&mut self, name: &'static str, ok: bool, fixable: bool, message: String) {
        if !ok {
            self.ok = false;
        }
        self.checks.push(DoctorCheck {
            name,
            ok,
            fixable,
            message,
        });
    }

    fn add_fix(&mut self, name: &'static str, message: String) {
        self.fixes_applied.push(DoctorFix { name, message });
    }
}

pub async fn run(paths: &AxonPaths, args: &DoctorArgs) -> Result<DoctorReport> {
    let mut report = DoctorReport::new(paths, args.fix);

    checks::check_state_root(paths, args, &mut report)?;
    identity_check::check_identity(paths, args, &mut report)?;
    checks::check_daemon_artifacts(paths, args, &mut report)?;
    checks::check_known_peers(paths, args, &mut report).await?;
    checks::check_config(paths, args, &mut report).await?;

    Ok(report)
}
