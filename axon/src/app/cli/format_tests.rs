use serde_json::json;

use super::{render_peers_human, render_status_human, render_whoami_human};

#[test]
fn peers_renderer_outputs_table_headers() {
    let output = render_peers_human(&json!({
        "ok": true,
        "peers": [{
            "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "public_key": "Zm9v",
            "trust": "enrolled",
            "locators": ["127.0.0.1:7100"],
            "status": "connected",
            "display_name": "alice"
        }]
    }))
    .expect("table output");

    assert!(output.contains("AGENT_ID"));
    assert!(output.contains("TRUST"));
    assert!(output.contains("enrolled"));
    assert!(output.contains("127.0.0.1:7100"));
    assert!(output.contains("alice"));
}

/// The bounded `peers` reply marks dropped detail with `truncated`; the
/// human path must surface it instead of printing a subset as complete.
#[test]
fn peers_renderer_discloses_truncation_for_retained_peers() {
    let output = render_peers_human(&json!({
        "ok": true,
        "truncated": true,
        "peers": [{
            "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "public_key": "Zm9v",
            "trust": "enrolled",
            "locators": [],
            "status": "enrolled",
            "display_name": null
        }]
    }))
    .expect("table output");

    assert!(
        output.contains("truncated"),
        "a truncated reply must be disclosed, never rendered as complete: {output}"
    );
}

/// The minimal bounded reply (not even a bare peer fits) must not read as
/// an empty directory.
#[test]
fn peers_renderer_discloses_truncation_with_no_retained_peers() {
    let output = render_peers_human(&json!({
        "ok": true,
        "truncated": true,
        "peers": []
    }))
    .expect("truncation notice");

    assert!(!output.contains("No peers."), "{output}");
    assert!(output.contains("truncated"), "{output}");
}

/// A complete reply keeps its plain rendering: no marker when nothing was
/// dropped.
#[test]
fn peers_renderer_stays_silent_when_complete() {
    let output = render_peers_human(&json!({
        "ok": true,
        "peers": [{
            "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "public_key": "Zm9v",
            "trust": "enrolled",
            "locators": ["127.0.0.1:7100"],
            "status": "connected",
            "display_name": "alice"
        }]
    }))
    .expect("table output");

    assert!(!output.contains("truncated"), "{output}");
}

#[test]
fn status_renderer_outputs_key_lines() {
    let output = render_status_human(&json!({
        "ok": true,
        "uptime_secs": 7,
        "peers_connected": 2,
        "messages_sent": 10,
        "messages_received": 4
    }))
    .expect("status output");

    assert!(output.contains("Uptime: 7s"));
    assert!(output.contains("Peers Connected: 2"));
}

#[test]
fn whoami_renderer_prints_name_or_unset() {
    let named = render_whoami_human(&json!({
        "ok": true,
        "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "public_key": "Zm9v",
        "name": "alice",
        "version": "0.1.0",
        "uptime_secs": 1
    }))
    .expect("whoami output");
    assert!(named.contains("Name: alice"));

    let unnamed = render_whoami_human(&json!({
        "ok": true,
        "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "public_key": "Zm9v",
        "version": "0.1.0",
        "uptime_secs": 1
    }))
    .expect("whoami output");
    assert!(unnamed.contains("Name: (unset)"));
}
