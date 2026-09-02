use serde_json::json;

use super::{MAX_MANIFEST_BYTES, MAX_SERVICES, Manifest, ServiceEntry};

fn sample_service(id: &str) -> ServiceEntry {
    ServiceEntry {
        id: id.to_string(),
        description: "Run cargo test on a workspace.".to_string(),
        example_request: Some(json!({"workspace": "/srv/axon"})),
        example_response: Some(json!({"passed": 214, "failed": 0})),
        timeout_hint_secs: Some(900),
        concurrency: Some(2),
        errors: Some(vec!["build_failed".to_string()]),
    }
}

fn sample_manifest() -> Manifest {
    Manifest::from_parts(
        Some("forge".to_string()),
        Some("0.9.0".to_string()),
        vec![sample_service("cargo_test")],
    )
    .expect("valid manifest")
}

#[test]
fn round_trips_through_json() {
    let manifest = sample_manifest();
    let value = manifest.to_payload_value().expect("payload");
    let parsed: Manifest =
        serde_json::from_value(value).expect("manifest re-parses from its own payload");
    assert_eq!(parsed, manifest);
}

#[test]
fn unknown_fields_are_ignored_for_forward_compatibility() {
    let value = json!({
        "name": "forge",
        "services": [{"id": "a", "description": "does a thing", "future_field": 1}],
        "also_future": true
    });
    let parsed: Manifest = serde_json::from_value(value).expect("unknown fields ignored");
    assert_eq!(parsed.services.len(), 1);
}

#[test]
fn requires_at_least_one_service() {
    let err = Manifest::from_parts(None, None, vec![]).expect_err("empty services rejected");
    assert!(err.to_string().contains("at least one service"));
}

#[test]
fn enforces_service_bound() {
    let services: Vec<_> = (0..=MAX_SERVICES)
        .map(|i| sample_service(&format!("svc{i}")))
        .collect();
    let err = Manifest::from_parts(None, None, services).expect_err("too many services rejected");
    assert!(err.to_string().contains("maximum is"));
}

#[test]
fn rejects_whitespace_in_service_ids() {
    let mut service = sample_service("has space");
    service.description = "fine".to_string();
    let err = Manifest::from_parts(None, None, vec![service]).expect_err("id rejected");
    assert!(err.to_string().contains("whitespace"));
}

#[test]
fn rejects_non_object_examples() {
    let mut service = sample_service("a");
    service.example_request = Some(json!("not an object"));
    let err = Manifest::from_parts(None, None, vec![service]).expect_err("example rejected");
    assert!(err.to_string().contains("example_request"));
}

#[test]
fn rejects_zero_and_oversized_concurrency() {
    let mut zero = sample_service("a");
    zero.concurrency = Some(0);
    assert!(Manifest::from_parts(None, None, vec![zero]).is_err());

    let mut huge = sample_service("b");
    huge.concurrency = Some(u32::MAX);
    assert!(Manifest::from_parts(None, None, vec![huge]).is_err());
}

#[test]
fn enforces_text_bounds() {
    let long_id = "x".repeat(129);
    let mut service = sample_service(&long_id);
    service.description = "fine".to_string();
    assert!(Manifest::from_parts(None, None, vec![service]).is_err());

    let mut service = sample_service("b");
    service.description = "y".repeat(2049);
    assert!(Manifest::from_parts(None, None, vec![service]).is_err());
}

#[test]
fn encoded_size_fits_the_wire_budget() {
    // A realistically dense manifest (16 services) must stay well inside the
    // encoded bound so a describe response can never exceed the wire limit.
    let services: Vec<_> = (0..16)
        .map(|i| sample_service(&format!("service_{i}")))
        .collect();
    let manifest =
        Manifest::from_parts(Some("forge".to_string()), None, services).expect("valid manifest");
    let size = manifest.encoded_size().expect("encoded");
    assert!(
        size < MAX_MANIFEST_BYTES,
        "size {size} must be < {MAX_MANIFEST_BYTES}"
    );
}

#[test]
fn schema_valid_but_oversized_manifest_is_rejected_at_parse() {
    // 16 services x 2 KiB descriptions ≈ 33 KiB: schema-valid, but over the
    // encoded bound. Parse must reject it — a manifest that parses always
    // satisfies every daemon invariant.
    let services: Vec<ServiceEntry> = (0..16)
        .map(|i| ServiceEntry {
            id: format!("service_{i}"),
            description: "d".repeat(2048),
            example_request: None,
            example_response: None,
            timeout_hint_secs: None,
            concurrency: None,
            errors: None,
        })
        .collect();
    // Build the JSON directly: from_parts would reject the shape before we
    // can observe the parse-time size bound.
    let value = json!({
        "name": "forge",
        "services": serde_json::to_value(&services).expect("services serialize"),
    });
    let err = serde_json::from_value::<Manifest>(value)
        .expect_err("oversized manifest must be rejected at parse");
    assert!(err.to_string().contains("maximum is 32768"));
}
