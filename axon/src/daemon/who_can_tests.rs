use serde_json::json;

use super::match_services;
use crate::manifest::{Manifest, ServiceEntry};

fn manifest_with_services(entries: &[(&str, &str)]) -> Manifest {
    let services: Vec<ServiceEntry> = entries
        .iter()
        .map(|(id, description)| ServiceEntry {
            id: (*id).to_string(),
            description: (*description).to_string(),
            example_request: Some(json!({})),
            example_response: None,
            timeout_hint_secs: None,
            concurrency: None,
            errors: None,
        })
        .collect();
    Manifest::from_parts(Some("forge".to_string()), None, services).expect("valid manifest")
}

#[test]
fn absent_query_lists_every_service() {
    let manifest = manifest_with_services(&[("cargo_test", "Runs tests"), ("lint", "Runs clippy")]);
    let matched = match_services(None, &manifest);
    assert_eq!(matched.len(), 2);
    assert_eq!(matched[0].id, "cargo_test");
}

#[test]
fn substring_matches_id_and_description_case_insensitively() {
    let manifest = manifest_with_services(&[
        ("cargo_test", "Runs the test suite"),
        ("lint", "Style checks"),
    ]);
    let by_id = match_services(Some("CARGO"), &manifest);
    assert_eq!(by_id.len(), 1);
    assert_eq!(by_id[0].id, "cargo_test");

    let by_description = match_services(Some("test suite"), &manifest);
    assert_eq!(by_description.len(), 1);
    assert_eq!(by_description[0].id, "cargo_test");

    let miss = match_services(Some("docker"), &manifest);
    assert!(miss.is_empty());
}

#[test]
fn whitespace_only_query_lists_everything() {
    let manifest = manifest_with_services(&[("a", "does a")]);
    assert_eq!(match_services(Some("   "), &manifest).len(), 1);
}
