use serde_json::json;

use super::super::tests::{REQUEST_TTL, agent, request};
use super::super::*;
use std::sync::Arc;

fn describe_request() -> Arc<Envelope> {
    Arc::new(Envelope::new(
        agent('a'),
        agent('b'),
        MessageKind::Describe,
        json!({}),
    ))
}

fn sample_manifest() -> Manifest {
    Manifest::from_parts(
        Some("forge".to_string()),
        Some("0.9.0".to_string()),
        vec![crate::manifest::ServiceEntry {
            id: "cargo_test".to_string(),
            description: "Run cargo test on a workspace.".to_string(),
            example_request: Some(json!({"workspace": "/srv/axon"})),
            example_response: None,
            timeout_hint_secs: Some(900),
            concurrency: Some(2),
            errors: Some(vec!["build_failed".to_string()]),
        }],
    )
    .expect("valid manifest")
}

#[tokio::test]
async fn describe_is_answered_from_registered_manifest_without_waking_handler() {
    let broker = RequestBroker::new(agent('b'));
    broker
        .register(1, Some(sample_manifest()))
        .await
        .expect("handler with manifest");

    let request = describe_request();
    let BeginRequest::Respond(response) = broker.begin(request, REQUEST_TTL).await else {
        panic!("describe must be answered by the broker, never delivered");
    };

    assert_eq!(response.kind, MessageKind::Response);
    let payload = response.payload_value().expect("payload");
    assert_eq!(payload["name"], "forge");
    assert_eq!(payload["services"][0]["id"], "cargo_test");
    // No pending entry may exist: the handler is never involved.
    assert_eq!(broker.pending_count().await, 0);
}

#[tokio::test]
async fn describe_without_manifest_returns_instructive_error() {
    let broker = RequestBroker::new(agent('b'));
    broker.register(1, None).await.expect("handler");

    let BeginRequest::Respond(response) = broker.begin(describe_request(), REQUEST_TTL).await
    else {
        panic!("describe must be answered by the broker, never delivered");
    };

    assert_eq!(response.kind, MessageKind::Error);
    let payload = response.payload_value().expect("payload");
    assert_eq!(payload["code"], "no_manifest");
    assert_eq!(payload["retryable"], json!(false));
    assert!(
        payload["message"]
            .as_str()
            .expect("message")
            .contains("serve with a manifest"),
        "error should teach the corrective action"
    );
}

#[tokio::test]
async fn describe_is_answered_without_any_handler() {
    let broker = RequestBroker::new(agent('b'));

    let BeginRequest::Respond(response) = broker.begin(describe_request(), REQUEST_TTL).await
    else {
        panic!("describe must be answered even without a handler");
    };
    assert_eq!(response.kind, MessageKind::Error);
    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "no_manifest"
    );
}

#[tokio::test]
async fn describe_answers_fresh_after_manifest_refresh() {
    let broker = RequestBroker::new(agent('b'));
    broker
        .register(1, Some(sample_manifest()))
        .await
        .expect("initial serve");

    let refreshed = Manifest::from_parts(
        Some("forge-v2".to_string()),
        None,
        vec![crate::manifest::ServiceEntry {
            id: "lint".to_string(),
            description: "Run clippy.".to_string(),
            example_request: None,
            example_response: None,
            timeout_hint_secs: None,
            concurrency: None,
            errors: None,
        }],
    )
    .expect("valid manifest");
    broker
        .register(1, Some(refreshed))
        .await
        .expect("idempotent re-serve");

    let BeginRequest::Respond(response) = broker.begin(describe_request(), REQUEST_TTL).await
    else {
        panic!("describe must be answered");
    };
    assert_eq!(
        response.payload_value().expect("payload")["name"],
        "forge-v2",
        "describe must bypass the completed-response tombstone cache"
    );
}

#[tokio::test]
async fn disconnect_clears_the_published_manifest() {
    let broker = RequestBroker::new(agent('b'));
    broker
        .register(1, Some(sample_manifest()))
        .await
        .expect("handler with manifest");
    broker.disconnect(1).await;

    let BeginRequest::Respond(response) = broker.begin(describe_request(), REQUEST_TTL).await
    else {
        panic!("describe must be answered");
    };
    assert_eq!(
        response.payload_value().expect("payload")["code"],
        "no_manifest",
        "manifest must not outlive its handler lease"
    );
}

#[tokio::test]
async fn describe_does_not_disturb_ordinary_request_flow() {
    let broker = RequestBroker::new(agent('b'));
    broker
        .register(1, Some(sample_manifest()))
        .await
        .expect("handler");

    let BeginRequest::Deliver(delivery) = broker.begin(request(), REQUEST_TTL).await else {
        panic!("ordinary requests must still be delivered to the handler");
    };
    assert_eq!(delivery.client_id, 1);
}
