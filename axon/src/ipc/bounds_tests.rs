//! Bounded-reply tests: `peers`/`who_can` must always fit the framed IPC
//! line limit even with legal adversarial state, and must mark omissions
//! explicitly (I6).

use serde_json::Value;

use super::{bounded_peers_reply, bounded_who_can_reply};
use crate::ipc::protocol::{
    DaemonReply, MAX_IPC_LINE_LENGTH, PeerSummary, ServiceMatch, ServiceSummary, encode_reply_line,
};

fn peer(agent_id: &str, locators: Vec<String>) -> PeerSummary {
    PeerSummary {
        agent_id: agent_id.to_string(),
        public_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
        trust: "enrolled",
        locators,
        status: "connected",
        rtt_ms: None,
        display_name: None,
        services: None,
    }
}

fn decode(reply: &DaemonReply) -> (Value, usize) {
    let line = encode_reply_line(reply).expect("a bounded reply must always encode");
    assert!(
        line.len() < MAX_IPC_LINE_LENGTH,
        "bounded reply must stay under the framed limit, got {} bytes",
        line.len()
    );
    (
        serde_json::from_str(&line).expect("bounded reply is JSON"),
        line.len(),
    )
}

// ---------------------------------------------------------------------------
// peers
// ---------------------------------------------------------------------------

/// Two peers with ~40 KiB host locators are legal enrollment state and made
/// the complete `peers` reply unanswerable (`message_too_large`) before the
/// reply was bounded by construction.
#[test]
fn peers_reply_with_overlong_locators_is_bounded_and_marked() {
    let reply = bounded_peers_reply(
        vec![
            peer(
                "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                vec![format!("{}.example:7100", "a".repeat(40_000))],
            ),
            peer(
                "ed25519.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                vec![format!("{}.example:7100", "b".repeat(40_000))],
            ),
        ],
        Some("req-1".to_string()),
    );

    let (decoded, _) = decode(&reply);
    assert_eq!(decoded["ok"], true);
    assert_eq!(
        decoded["truncated"], true,
        "the omission must be explicit, never silent"
    );
    assert_eq!(decoded["req_id"], "req-1", "correlation must survive");
    assert_eq!(
        decoded["peers"].as_array().map(Vec::len),
        Some(2),
        "peer identities are the most valuable data and must be kept"
    );
}

/// Even a single peer whose locators alone exceed the line limit must not
/// make the command unanswerable.
#[test]
fn peers_reply_with_one_pathological_peer_is_still_answered() {
    let reply = bounded_peers_reply(
        vec![peer(
            "ed25519.cccccccccccccccccccccccccccccccc",
            vec![format!("{}.example:7100", "c".repeat(200_000))],
        )],
        None,
    );

    let (decoded, _) = decode(&reply);
    assert_eq!(decoded["truncated"], true);
    assert_eq!(decoded["peers"].as_array().map(Vec::len), Some(1));
}

/// A bounded listing that cannot even hold every peer's identity drops
/// trailing peers and still fits: the peer count itself is variable.
#[test]
fn peers_reply_bounds_the_number_of_listed_peers() {
    let peers: Vec<PeerSummary> = (0..512)
        .map(|index| {
            peer(
                &format!("ed25519.{index:032x}"),
                vec![format!("{}.example:7100", "x".repeat(200))],
            )
        })
        .collect();
    let reply = bounded_peers_reply(peers, None);

    let (decoded, _) = decode(&reply);
    assert_eq!(decoded["truncated"], true);
    let listed = decoded["peers"].as_array().map(Vec::len).unwrap_or(0);
    assert!(listed < 512, "the listing must be bounded");
}

/// The common case is untouched: a complete reply keeps the documented
/// `peers` shape (a distinct variant only when bounding occurred).
#[test]
fn complete_peers_reply_keeps_its_shape() {
    let reply = bounded_peers_reply(
        vec![peer(
            "ed25519.dddddddddddddddddddddddddddddddd",
            vec!["127.0.0.1:7100".to_string()],
        )],
        None,
    );
    assert!(matches!(reply, DaemonReply::Peers { .. }));
    let value = serde_json::to_value(&reply).expect("json");
    assert!(
        value.get("truncated").is_none(),
        "the complete reply must not grow a truncation marker"
    );
}

/// An in-process caller can bypass the ingress req_id bound; the bounded
/// reply must still fit and drop the overbound echo rather than fail.
#[test]
fn peers_reply_never_exceeds_the_limit_even_with_an_overbound_req_id() {
    let reply = bounded_peers_reply(
        vec![peer(
            "ed25519.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            vec!["127.0.0.1:7100".to_string()],
        )],
        Some("r".repeat(70_000)),
    );

    let (decoded, _) = decode(&reply);
    assert!(
        decoded.get("req_id").is_none(),
        "the overbound echo must be dropped, never truncated"
    );
}

// ---------------------------------------------------------------------------
// who_can
// ---------------------------------------------------------------------------

/// Every connected peer may contribute a full manifest, so a large fleet can
/// push `who_can` past the line limit; the reply must be capped explicitly.
#[test]
fn who_can_reply_with_many_manifests_is_bounded_and_marked() {
    let matches: Vec<ServiceMatch> = (0..256)
        .map(|index| ServiceMatch {
            agent_id: format!("ed25519.{index:032x}"),
            services: vec![ServiceSummary {
                id: format!("svc-{index}"),
                description: "d".repeat(1024),
            }],
        })
        .collect();
    let unreachable: Vec<String> = (0..256)
        .map(|index| format!("ed25519.{index:032x}"))
        .collect();

    let reply = bounded_who_can_reply(matches, unreachable, Some("req-2".to_string()));

    let (decoded, _) = decode(&reply);
    assert_eq!(decoded["truncated"], true);
    assert_eq!(decoded["req_id"], "req-2");
    assert!(
        !decoded["matches"]
            .as_array()
            .expect("matches array")
            .is_empty(),
        "matching services are the answer and must be kept first"
    );
}

/// A single match near the manifest bound still fits whole; the bound only
/// engages when the total reply overflows.
#[test]
fn complete_who_can_reply_keeps_its_shape() {
    let reply = bounded_who_can_reply(
        vec![ServiceMatch {
            agent_id: "ed25519.ffffffffffffffffffffffffffffffff".to_string(),
            services: vec![ServiceSummary {
                id: "cargo_test".to_string(),
                description: "Run cargo test.".to_string(),
            }],
        }],
        vec!["ed25519.11111111111111111111111111111111".to_string()],
        None,
    );
    assert!(matches!(
        reply,
        DaemonReply::WhoCan {
            truncated: false,
            ..
        }
    ));
    let value = serde_json::to_value(&reply).expect("json");
    assert!(
        value.get("truncated").is_none(),
        "the complete reply must not grow a truncation marker"
    );
}

#[test]
fn who_can_reply_never_exceeds_the_limit_even_with_an_overbound_req_id() {
    let reply = bounded_who_can_reply(
        vec![ServiceMatch {
            agent_id: "ed25519.22222222222222222222222222222222".to_string(),
            services: vec![],
        }],
        Vec::new(),
        Some("r".repeat(70_000)),
    );

    let (decoded, _) = decode(&reply);
    assert!(decoded.get("req_id").is_none());
}
