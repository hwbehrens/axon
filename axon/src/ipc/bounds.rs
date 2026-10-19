//! Bounded construction of variable-length IPC replies.
//!
//! The size of `peers` and `who_can` replies is set by directory and
//! manifest state, not by the caller: up to `MAX_ENROLLED_PEERS +
//! MAX_CANDIDATE_PEERS` peers with unbounded locator strings, or every
//! connected peer's manifest (up to `MAX_MANIFEST_BYTES` each). Without a
//! bound, a legal state made those commands permanently unanswerable: the
//! encoder refused the frame with `message_too_large` every time (I6).
//!
//! These constructors bound the reply BY CONSTRUCTION — dropping the least
//! valuable detail first and marking the omission explicitly with
//! `truncated: true` — so the framed line always fits. Frames themselves are
//! still never truncated (DEC-022/DEC-023).

use super::protocol::{
    DaemonReply, MAX_IPC_LINE_LENGTH, PeerSummary, ServiceMatch, encode_reply_line,
};

/// Compose a `peers` reply that always encodes within one framed IPC line.
///
/// The complete listing is returned unchanged when it fits. Otherwise
/// advisory detail is dropped from each peer in order of decreasing value
/// (manifest service ids, then locators, then display names) and whole
/// trailing peers are omitted; the bounding constructor returns
/// [`DaemonReply::PeersBounded`] with an explicit `truncated` marker.
pub fn bounded_peers_reply(peers: Vec<PeerSummary>, req_id: Option<String>) -> DaemonReply {
    let complete = DaemonReply::Peers {
        ok: true,
        peers: peers.clone(),
        req_id: req_id.clone(),
    };
    if encode_reply_line(&complete).is_ok() {
        return complete;
    }

    // `encode_reply_line` permits a body of MAX-1 bytes: the trailing
    // newline is the last byte of the frame.
    let budget = MAX_IPC_LINE_LENGTH - 1;
    let mut used = match serde_json::to_string(&DaemonReply::PeersBounded {
        ok: true,
        peers: Vec::new(),
        truncated: true,
        req_id: req_id.clone(),
    }) {
        Ok(skeleton) => skeleton.len(),
        // Unreachable for plain JSON; fall back to the minimal listing
        // rather than emit anything unbounded.
        Err(_) => return minimal_peers_reply(),
    };
    if used > budget {
        // Only reachable for an in-process caller that bypassed the
        // MAX_REQ_ID_BYTES ingress bound: never echo an overbound id.
        return minimal_peers_reply();
    }

    let mut kept = Vec::with_capacity(peers.len());
    let mut truncated = false;
    for mut peer in peers {
        // Drop this peer's least valuable fields until it fits the budget.
        loop {
            let encoded = match serde_json::to_string(&peer) {
                Ok(encoded) => encoded,
                Err(_) => {
                    truncated = true;
                    break;
                }
            };
            // `used` already counts the empty array's two framing bytes, so
            // the +1 element separator over-estimates by one byte: the
            // estimate is conservative and the fit check exact-or-pessimistic.
            if used + 1 + encoded.len() <= budget {
                used += 1 + encoded.len();
                kept.push(peer);
                break;
            }
            truncated = true;
            if peer.services.is_some() {
                peer.services = None;
            } else if !peer.locators.is_empty() {
                peer.locators.clear();
            } else if peer.display_name.is_some() {
                peer.display_name = None;
            } else {
                // Cannot fit even bare: omit the peer from the listing.
                break;
            }
        }
    }

    DaemonReply::PeersBounded {
        ok: true,
        peers: kept,
        truncated,
        req_id,
    }
}

/// The always-fitting fallback for [`bounded_peers_reply`]; the echo is
/// dropped because it is the only unbounded field.
fn minimal_peers_reply() -> DaemonReply {
    DaemonReply::PeersBounded {
        ok: true,
        peers: Vec::new(),
        truncated: true,
        req_id: None,
    }
}

/// Compose a `who_can` reply that always encodes within one framed IPC line.
///
/// Complete matches win over trailing matches, whole matches win over
/// partial ones, and partial matches keep their leading services. A match is
/// never emitted with an empty `services` array: a match answers "this peer
/// exposes these services", so a partial match with no retainable service is
/// dropped whole rather than claiming an empty match. The diagnostic
/// `unreachable` list is bounded last. Anything dropped sets
/// `truncated: true` on the reply.
pub fn bounded_who_can_reply(
    matches: Vec<ServiceMatch>,
    unreachable: Vec<String>,
    req_id: Option<String>,
) -> DaemonReply {
    let complete = DaemonReply::WhoCan {
        ok: true,
        matches: matches.clone(),
        unreachable: unreachable.clone(),
        truncated: false,
        req_id: req_id.clone(),
    };
    if encode_reply_line(&complete).is_ok() {
        return complete;
    }

    let budget = MAX_IPC_LINE_LENGTH - 1;
    let mut used = match serde_json::to_string(&DaemonReply::WhoCan {
        ok: true,
        matches: Vec::new(),
        unreachable: Vec::new(),
        truncated: true,
        req_id: req_id.clone(),
    }) {
        Ok(skeleton) => skeleton.len(),
        Err(_) => return minimal_who_can_reply(),
    };
    if used > budget {
        // Only reachable for an in-process caller that bypassed the
        // MAX_REQ_ID_BYTES ingress bound: never echo an overbound id.
        return minimal_who_can_reply();
    }

    let mut kept_matches: Vec<ServiceMatch> = Vec::with_capacity(matches.len());
    let mut kept_unreachable: Vec<String> = Vec::new();
    let mut truncated = false;
    for candidate in matches {
        // Whole-match fast path: one manifest is size-bounded, so at least
        // the first match always fits unless the budget is already spent.
        if let Ok(encoded) = serde_json::to_string(&candidate)
            && used + 1 + encoded.len() <= budget
        {
            used += 1 + encoded.len();
            kept_matches.push(candidate);
            continue;
        }
        truncated = true;

        // Partial match: keep the leading services that still fit.
        let bare = ServiceMatch {
            agent_id: candidate.agent_id,
            services: Vec::new(),
        };
        let Ok(bare_encoded) = serde_json::to_string(&bare) else {
            break;
        };
        let bare_len = bare_encoded.len();
        if used + 1 + bare_len > budget {
            // No later match can fit either: the per-match framing alone
            // already exceeds the remaining budget.
            break;
        }
        let mut match_len = bare_len;
        let mut services = Vec::new();
        for service in candidate.services {
            let Ok(encoded) = serde_json::to_string(&service) else {
                break;
            };
            // `match_len` includes the empty `"services":[]`, so each kept
            // service over-estimates by one byte — conservative.
            let next = match_len + 1 + encoded.len();
            if used + 1 + next > budget {
                break;
            }
            match_len = next;
            services.push(service);
        }
        if services.is_empty() {
            // The identity alone fits but not one matching service: keep
            // scanning instead of emitting a match with no services, which
            // clients would read as "this peer matched but exposes nothing".
            continue;
        }
        used += 1 + match_len;
        kept_matches.push(ServiceMatch {
            agent_id: bare.agent_id,
            services,
        });
    }

    for name in unreachable {
        let Ok(encoded) = serde_json::to_string(&name) else {
            truncated = true;
            break;
        };
        if used + 1 + encoded.len() <= budget {
            used += 1 + encoded.len();
            kept_unreachable.push(name);
        } else {
            truncated = true;
            // Prefix semantics: stop rather than scan for shorter names.
            break;
        }
    }

    DaemonReply::WhoCan {
        ok: true,
        matches: kept_matches,
        unreachable: kept_unreachable,
        truncated,
        req_id,
    }
}

/// The always-fitting fallback for [`bounded_who_can_reply`]; the echo is
/// dropped because it is the only unbounded field.
fn minimal_who_can_reply() -> DaemonReply {
    DaemonReply::WhoCan {
        ok: true,
        matches: Vec::new(),
        unreachable: Vec::new(),
        truncated: true,
        req_id: None,
    }
}

#[cfg(test)]
#[path = "bounds_tests.rs"]
mod tests;
