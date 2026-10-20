//! Regression coverage for bounded endpoint dialing:
//!
//! - Configured locators are dialed before unvalidated observed candidates,
//!   so a blackholed observation can never starve an explicit locator.
//! - Per-endpoint effort is capped to a fair share of the remaining budget,
//!   so a blackholed address cannot consume the whole exchange.
//! - A retry is only attempted while budget remains; otherwise the
//!   informative first error is preserved.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::tempdir;

use super::fixtures::make_transport_pair;
use crate::config::AxonPaths;
use crate::identity::Identity;
use crate::message::{AgentId, Envelope, MessageKind};
use crate::peer_directory::{
    ObservationId, ObservationSource, PeerIdentity, PeerLocator, PeerObservation,
};

/// UDP sink that accepts datagrams and never answers, so a QUIC handshake to
/// it can only time out. The caller keeps the socket alive as a blackhole.
fn blackhole_udp() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").expect("bind blackhole")
}

#[test]
fn observed_candidates_never_precede_configured_locators() {
    let configured: SocketAddr = "127.0.0.1:55000".parse().unwrap();
    let observed_lower: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let observed_higher: SocketAddr = "127.0.0.1:60000".parse().unwrap();

    let ordered = super::super::dial::order_dial_addresses(
        vec![configured],
        vec![observed_higher, observed_lower],
    );
    assert_eq!(
        ordered,
        vec![configured, observed_lower, observed_higher],
        "configured locators must be dialed first, observed candidates sorted behind them"
    );
}

#[tokio::test]
async fn blackholed_endpoints_do_not_starve_a_live_configured_locator() {
    let pair = make_transport_pair().await;
    let agent_b = AgentId::parse(pair.id_b.agent_id()).unwrap();
    let live_addr = pair.transport_b.local_addr().unwrap();

    // Worst-case pre-fix ordering: two blackholes, both lower-sorted than
    // the live endpoint. One is observed (unvalidated), one configured.
    let mut sinks = Vec::new();
    while sinks.len() < 2 {
        let sink = blackhole_udp();
        if sink.local_addr().unwrap().port() < live_addr.port() {
            sinks.push(sink);
        }
    }
    sinks.sort_by_key(|sink| sink.local_addr().unwrap().port());
    let observed_blackhole = sinks[0].local_addr().unwrap();
    let configured_blackhole = sinks[1].local_addr().unwrap();
    assert!(observed_blackhole < configured_blackhole && configured_blackhole < live_addr);

    let identity_b = PeerIdentity::from_public_key(pair.id_b.public_key_base64()).unwrap();
    pair.directory_a
        .enroll(identity_b, vec![PeerLocator::Socket(configured_blackhole)])
        .await
        .expect("add blackholed configured locator");
    pair.directory_a
        .observe(
            PeerObservation::new(
                ObservationId::new(format!("test:blackhole:{observed_blackhole}")).unwrap(),
                agent_b.clone(),
                pair.id_b.public_key_base64(),
                Some(observed_blackhole),
                None,
                ObservationSource::Mdns,
            )
            .unwrap(),
        )
        .await;

    let message = Envelope::new(
        AgentId::parse(pair.id_a.agent_id()).unwrap(),
        agent_b.clone(),
        MessageKind::Message,
        json!({"probe": true}),
    );
    let started = Instant::now();
    pair.transport_a
        .send_to(&pair.directory_a, &agent_b, message, Duration::from_secs(2))
        .await
        .expect("live configured locator must be reached despite blackholes");
    assert!(
        started.elapsed() < Duration::from_millis(1900),
        "blackholed endpoints consumed the whole exchange budget: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn exhausted_budget_preserves_the_first_error() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();

    // Enroll a peer whose only locator is a UDP blackhole: the first attempt
    // can only time out, consuming the whole exchange budget.
    let dir_c = tempdir().expect("tempdir c");
    let paths_c = AxonPaths::from_root(PathBuf::from(dir_c.path()));
    let id_c = Identity::load_or_generate(&paths_c).expect("identity c");
    let agent_c = AgentId::parse(id_c.agent_id()).unwrap();
    let identity_c = PeerIdentity::from_public_key(id_c.public_key_base64()).unwrap();
    let sink = blackhole_udp();
    pair.directory_a
        .enroll(
            identity_c,
            vec![PeerLocator::Socket(sink.local_addr().unwrap())],
        )
        .await
        .expect("enroll blackholed peer");

    let request = Envelope::new(
        agent_a,
        agent_c.clone(),
        MessageKind::Request,
        json!({"question": "anyone?"}),
    );
    let started = Instant::now();
    let err = pair
        .transport_a
        .send_to(&pair.directory_a, &agent_c, request, Duration::from_secs(2))
        .await
        .expect_err("blackholed dial must fail");
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "first attempt must have consumed the budget: {:?}",
        started.elapsed()
    );
    assert!(
        err.to_string().contains("QUIC handshake timed out"),
        "budget-exhausted retry must not replace the informative first error: {err}"
    );
    // R-DIAL: the handshake deadline is budget exhaustion, so the failure
    // must stay classified as a timeout (spec/IPC.md §5, Q-011). Before the
    // fix every `dial()` error was rewrapped as `pre_send` (`timed_out`
    // false) and the daemon reported `peer_unreachable`.
    assert!(
        err.timed_out,
        "handshake budget exhaustion must stay timed_out: {err}"
    );
    assert!(
        !err.ambiguous,
        "no payload byte was written, so the failure is not ambiguous: {err}"
    );
}

/// The fire-and-forget (message) kind uses the same dial path, so its
/// classification must not depend on the kind: a dial that exhausts its
/// deadline is still `timed_out` (the daemon then maps it to `timeout`).
#[tokio::test]
async fn message_dial_timeout_keeps_the_timeout_classification() {
    let pair = make_transport_pair().await;
    let agent_a = AgentId::parse(pair.id_a.agent_id()).unwrap();

    let dir_c = tempdir().expect("tempdir c");
    let paths_c = AxonPaths::from_root(PathBuf::from(dir_c.path()));
    let id_c = Identity::load_or_generate(&paths_c).expect("identity c");
    let agent_c = AgentId::parse(id_c.agent_id()).unwrap();
    let identity_c = PeerIdentity::from_public_key(id_c.public_key_base64()).unwrap();
    let sink = blackhole_udp();
    pair.directory_a
        .enroll(
            identity_c,
            vec![PeerLocator::Socket(sink.local_addr().unwrap())],
        )
        .await
        .expect("enroll blackholed peer");

    let message = Envelope::new(
        agent_a,
        agent_c.clone(),
        MessageKind::Message,
        json!({"note": "fire and forget"}),
    );
    let err = pair
        .transport_a
        .send_to(&pair.directory_a, &agent_c, message, Duration::from_secs(2))
        .await
        .expect_err("blackholed dial must fail");
    assert!(
        err.timed_out,
        "a message dial timeout must stay timed_out: {err}"
    );
    assert!(!err.ambiguous, "the message never reached the wire: {err}");
}
