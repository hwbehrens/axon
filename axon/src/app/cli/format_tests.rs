use serde_json::json;

use super::{render_peers_human, render_status_human, render_whoami_human};

#[test]
fn peers_renderer_outputs_table_headers() {
    let output = render_peers_human(&json!({
        "ok": true,
        "peers": [{
            "agent_id": "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "addr": "127.0.0.1:7100",
            "status": "connected",
            "rtt_ms": 1.25,
            "source": "static"
        }]
    }))
    .expect("table output");

    assert!(output.contains("AGENT_ID"));
    assert!(output.contains("127.0.0.1:7100"));
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
