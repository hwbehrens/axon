use anyhow::Result;

use axon::config::AxonPaths;
use axon::peer_directory::PeerStore;

use crate::app::doctor::{DoctorArgs, DoctorReport};

use super::backup_file_with_timestamp;

pub(in crate::app::doctor) async fn check_peer_store(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if paths.legacy_known_peers.exists() {
        if args.fix {
            let backup = backup_file_with_timestamp(&paths.legacy_known_peers)?;
            report.add_fix(
                "legacy_peer_state_removed",
                format!(
                    "moved unsupported known_peers.json to {}; peers must be intentionally re-enrolled",
                    backup.display()
                ),
            );
            report.add_check(
                "legacy_peer_state",
                true,
                true,
                "unsupported legacy peer cache moved aside".to_string(),
            );
        } else {
            report.add_check(
                "legacy_peer_state",
                false,
                true,
                "known_peers.json is unsupported; run `axon doctor --fix`, then intentionally re-enroll peers"
                    .to_string(),
            );
        }
    } else {
        report.add_check(
            "legacy_peer_state",
            true,
            false,
            "no unsupported legacy peer cache".to_string(),
        );
    }

    if !paths.peers.exists() {
        report.add_check(
            "peer_store",
            true,
            false,
            "peers.json not present (no peers enrolled)".to_string(),
        );
        return Ok(());
    }

    match PeerStore::new(paths.peers.clone()).validate().await {
        Ok(count) => report.add_check(
            "peer_store",
            true,
            false,
            format!("peers.json parsed and validated ({count} enrolled peers)"),
        ),
        Err(err) => report.add_check(
            "peer_store",
            false,
            false,
            format!(
                "peers.json is invalid ({err}); AXON will fail closed. Restore a trusted backup or remove it and re-enroll peers"
            ),
        ),
    }
    Ok(())
}
