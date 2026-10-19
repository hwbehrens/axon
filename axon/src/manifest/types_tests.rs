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

/// Hand-transcribed literal of the normative schema example in
/// spec/WIRE_FORMAT.md §6.5. It is deliberately NOT produced by serializing
/// the structs under test: a field rename must fail here.
const SPEC_6_5_EXAMPLE: &str = r#"
{
  "name": "forge",
  "version": "0.9.0",
  "services": [
    {
      "id": "cargo_test",
      "description": "Run cargo test on a workspace; returns condensed failure output.",
      "example_request": {"workspace": "/srv/axon"},
      "example_response": {"passed": 214, "failed": 0},
      "timeout_hint_secs": 900,
      "concurrency": 2,
      "errors": ["build_failed"]
    }
  ]
}
"#;

#[test]
fn spec_6_5_example_transcribes_every_schema_field() {
    let manifest: Manifest =
        serde_json::from_str(SPEC_6_5_EXAMPLE).expect("spec §6.5 example must deserialize");
    assert_eq!(manifest.name.as_deref(), Some("forge"));
    assert_eq!(manifest.version.as_deref(), Some("0.9.0"));
    assert_eq!(manifest.services.len(), 1);
    let service = &manifest.services[0];
    assert_eq!(service.id, "cargo_test");
    assert_eq!(
        service.description,
        "Run cargo test on a workspace; returns condensed failure output."
    );
    assert_eq!(
        service.example_request,
        Some(json!({"workspace": "/srv/axon"}))
    );
    assert_eq!(
        service.example_response,
        Some(json!({"passed": 214, "failed": 0}))
    );
    assert_eq!(service.timeout_hint_secs, Some(900));
    assert_eq!(service.concurrency, Some(2));
    assert_eq!(service.errors, Some(vec!["build_failed".to_string()]));

    // Re-serialization must reproduce the spec's example field for field
    // (names and values), so a rename on either side fails this test.
    let reencoded = serde_json::to_value(&manifest).expect("manifest serializes");
    let spec_service: serde_json::Value =
        serde_json::from_str(SPEC_6_5_EXAMPLE).expect("fixture is JSON");
    assert_eq!(reencoded["services"][0], spec_service["services"][0]);
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
fn enforces_name_and_version_bounds() {
    let service = || vec![sample_service("a")];
    assert!(Manifest::from_parts(Some(String::new()), None, service()).is_err());
    assert!(Manifest::from_parts(Some("n".repeat(129)), None, service()).is_err());
    Manifest::from_parts(Some("n".repeat(128)), None, service()).expect("128-byte name allowed");
    assert!(Manifest::from_parts(None, Some(String::new()), service()).is_err());
    assert!(Manifest::from_parts(None, Some("v".repeat(65)), service()).is_err());
    Manifest::from_parts(None, Some("v".repeat(64)), service()).expect("64-byte version allowed");
}

#[test]
fn enforces_service_text_minimums_and_error_code_bounds() {
    let mut empty_id = sample_service("");
    empty_id.description = "fine".to_string();
    assert!(Manifest::from_parts(None, None, vec![empty_id]).is_err());

    let mut empty_description = sample_service("a");
    empty_description.description = String::new();
    assert!(Manifest::from_parts(None, None, vec![empty_description]).is_err());

    let mut too_many_errors = sample_service("b");
    too_many_errors.errors = Some((0..33).map(|i| format!("e{i}")).collect());
    assert!(Manifest::from_parts(None, None, vec![too_many_errors]).is_err());

    let mut empty_code = sample_service("c");
    empty_code.errors = Some(vec![String::new()]);
    assert!(Manifest::from_parts(None, None, vec![empty_code]).is_err());

    let mut long_code = sample_service("d");
    long_code.errors = Some(vec!["x".repeat(65)]);
    assert!(Manifest::from_parts(None, None, vec![long_code]).is_err());

    let mut at_bounds = sample_service("e");
    at_bounds.errors = Some((0..32).map(|i| format!("e{i:03}")).collect());
    Manifest::from_parts(None, None, vec![at_bounds]).expect("32 error codes of 4 bytes allowed");
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
