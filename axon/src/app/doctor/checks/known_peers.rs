use std::collections::{HashMap, HashSet};

use anyhow::Result;

use axon::config::{AxonPaths, Config, load_known_peers, save_known_peers};

use crate::app::doctor::{DoctorArgs, DoctorReport};

use super::backup_file_with_timestamp;

pub(in crate::app::doctor) async fn check_known_peers(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if !paths.known_peers.exists() {
        report.add_check(
            "known_peers",
            true,
            false,
            "known_peers.json not present".to_string(),
        );
        return Ok(());
    }

    match load_known_peers(&paths.known_peers).await {
        Ok(peers) => {
            report.add_check(
                "known_peers",
                true,
                false,
                format!("known_peers.json parsed ({} entries)", peers.len()),
            );
        }
        Err(err) => {
            if args.fix {
                let backup = backup_file_with_timestamp(&paths.known_peers)?;
                save_known_peers(&paths.known_peers, &[]).await?;
                report.add_fix(
                    "known_peers_reset",
                    format!(
                        "backed up corrupt known_peers.json to {} and reset to []",
                        backup.display()
                    ),
                );
                report.add_check(
                    "known_peers",
                    true,
                    true,
                    "corrupt known_peers.json reset".to_string(),
                );
            } else {
                report.add_check(
                    "known_peers",
                    false,
                    true,
                    format!(
                        "known_peers.json is not parseable ({err}); run `axon doctor --fix` to back up and reset"
                    ),
                );
            }
        }
    }

    Ok(())
}

pub(in crate::app::doctor) async fn check_duplicate_peer_addrs(
    paths: &AxonPaths,
    args: &DoctorArgs,
    report: &mut DoctorReport,
) -> Result<()> {
    if !paths.known_peers.exists() {
        report.add_check(
            "duplicate_peer_addr",
            true,
            false,
            "known_peers.json not present".to_string(),
        );
        return Ok(());
    }
    let peers = match load_known_peers(&paths.known_peers).await {
        Ok(p) => p,
        Err(_) => {
            report.add_check(
                "duplicate_peer_addr",
                true,
                false,
                "known_peers.json not parseable (already reported)".to_string(),
            );
            return Ok(());
        }
    };

    let mut by_addr: HashMap<std::net::SocketAddr, Vec<_>> = HashMap::new();
    for peer in &peers {
        by_addr.entry(peer.addr).or_default().push(peer);
    }

    let static_ids: HashSet<String> = Config::load(&paths.config)
        .await
        .map(|cfg| cfg.peers.iter().map(|p| p.agent_id.to_string()).collect())
        .unwrap_or_default();

    let mut duplicates_found = false;
    let mut removed_ids: Vec<String> = Vec::new();
    for (addr, group) in &by_addr {
        if group.len() < 2 {
            continue;
        }
        duplicates_found = true;
        if args.fix {
            let keeper = group
                .iter()
                .find(|p| static_ids.contains(p.agent_id.as_str()))
                .or_else(|| group.iter().max_by_key(|p| p.last_seen_unix_ms))
                .expect("group is non-empty");
            for peer in group.iter().filter(|p| p.agent_id != keeper.agent_id) {
                removed_ids.push(peer.agent_id.to_string());
                report.add_fix(
                    "duplicate_addr_prune",
                    format!(
                        "removed stale peer {} at {} (superseded by {})",
                        peer.agent_id, addr, keeper.agent_id
                    ),
                );
            }
        } else {
            let details: Vec<String> = group
                .iter()
                .map(|p| {
                    let source = if static_ids.contains(p.agent_id.as_str()) {
                        "static"
                    } else {
                        "cached"
                    };
                    format!(
                        "{} ({}, last_seen: {})",
                        p.agent_id, source, p.last_seen_unix_ms
                    )
                })
                .collect();
            report.add_check(
                "duplicate_peer_addr",
                false,
                true,
                format!(
                    "duplicate address {}: [{}]; run `axon doctor --fix` to prune",
                    addr,
                    details.join(", ")
                ),
            );
        }
    }

    if args.fix && !removed_ids.is_empty() {
        let backup = backup_file_with_timestamp(&paths.known_peers)?;
        let filtered: Vec<_> = peers
            .into_iter()
            .filter(|p| !removed_ids.contains(&p.agent_id.to_string()))
            .collect();
        save_known_peers(&paths.known_peers, &filtered).await?;
        report.add_check(
            "duplicate_peer_addr",
            true,
            true,
            format!(
                "pruned {} stale duplicate(s); backup at {}",
                removed_ids.len(),
                backup.display()
            ),
        );
    } else if duplicates_found && args.fix {
        report.add_check(
            "duplicate_peer_addr",
            true,
            false,
            "duplicate addresses found but all entries are static".to_string(),
        );
    } else if !duplicates_found {
        report.add_check(
            "duplicate_peer_addr",
            true,
            false,
            "no duplicate peer addresses".to_string(),
        );
    }
    Ok(())
}
