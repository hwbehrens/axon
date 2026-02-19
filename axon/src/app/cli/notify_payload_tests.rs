use serde_json::json;

use super::parse_notify_payload;

#[test]
fn default_mode_wraps_literal_text() {
    let value = parse_notify_payload("{bad", false).expect("default mode should keep literal text");
    assert_eq!(value, json!("{bad"));
}

#[test]
fn json_mode_accepts_structured_json() {
    let value = parse_notify_payload("{\"x\":1}", true).expect("valid json");
    assert_eq!(value, json!({"x": 1}));
}

#[test]
fn json_mode_rejects_invalid_json() {
    let err = parse_notify_payload("{bad", true).expect_err("invalid json should fail");
    assert!(err.to_string().contains("invalid JSON"));
}
