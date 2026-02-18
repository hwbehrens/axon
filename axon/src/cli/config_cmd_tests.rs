use super::{ConfigArgs, ConfigKey, apply_set, parse_action, render_list_text};
use axon::config::PersistedConfig;

#[test]
fn parse_action_rejects_json_without_list() {
    let args = ConfigArgs {
        list: false,
        unset: None,
        edit: false,
        json: true,
        key: Some(ConfigKey::Name),
        value: None,
    };

    let err = parse_action(&args).expect_err("--json without --list should fail");
    assert!(err.to_string().contains("only supported with --list"));
}

#[test]
fn apply_set_validates_name_port_and_addr() {
    let mut config = PersistedConfig::default();

    apply_set(&mut config, ConfigKey::Name, " Alice ").expect("name");
    apply_set(&mut config, ConfigKey::Port, "7200").expect("port");
    apply_set(&mut config, ConfigKey::AdvertiseAddr, "example.com:7100").expect("addr");

    assert_eq!(config.name.as_deref(), Some("Alice"));
    assert_eq!(config.port, Some(7200));
    assert_eq!(config.advertise_addr.as_deref(), Some("example.com:7100"));
}

#[test]
fn apply_set_rejects_bad_values() {
    let mut config = PersistedConfig::default();
    assert!(apply_set(&mut config, ConfigKey::Name, "   ").is_err());
    assert!(apply_set(&mut config, ConfigKey::Port, "not-a-number").is_err());
    assert!(apply_set(&mut config, ConfigKey::AdvertiseAddr, "missing-port").is_err());
}

#[test]
fn render_list_text_only_includes_set_keys() {
    let config = PersistedConfig {
        name: Some("alice".to_string()),
        port: None,
        advertise_addr: Some("host:7100".to_string()),
        peers: Vec::new(),
    };

    let rendered = render_list_text(&config);
    assert!(rendered.contains("name=alice"));
    assert!(rendered.contains("advertise_addr=host:7100"));
    assert!(!rendered.contains("port="));
}
