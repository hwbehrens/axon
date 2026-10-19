use clap::{Args as _, FromArgMatches as _};

use super::{ConfigArgs, ConfigKey, apply_set, key_display_name, parse_action, render_list_text};
use axon::config::PersistedConfig;

fn parse_config_args(argv: &[&str]) -> Result<ConfigArgs, clap::Error> {
    let matches =
        ConfigArgs::augment_args(clap::Command::new("config")).try_get_matches_from(argv)?;
    ConfigArgs::from_arg_matches(&matches)
}

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
fn apply_set_rejects_port_zero() {
    let mut config = PersistedConfig::default();
    let err = apply_set(&mut config, ConfigKey::Port, "0").expect_err("port 0 should fail");
    assert!(err.to_string().contains("port 0"));
}

#[test]
fn apply_set_accepts_valid_port_boundaries() {
    let mut config = PersistedConfig::default();
    apply_set(&mut config, ConfigKey::Port, "1").expect("port 1");
    assert_eq!(config.port, Some(1));
    apply_set(&mut config, ConfigKey::Port, "65535").expect("port 65535");
    assert_eq!(config.port, Some(65535));
}

#[test]
fn key_display_name_maps_all_keys() {
    assert_eq!(key_display_name(ConfigKey::Name), "name");
    assert_eq!(key_display_name(ConfigKey::Port), "port");
    assert_eq!(key_display_name(ConfigKey::AdvertiseAddr), "advertise_addr");
}

#[test]
fn config_key_accepts_both_advertise_spellings() {
    let underscore =
        parse_config_args(&["config", "advertise_addr", "host:7100"]).expect("underscore key");
    assert_eq!(underscore.key, Some(ConfigKey::AdvertiseAddr));
    assert_eq!(underscore.value.as_deref(), Some("host:7100"));

    let hyphen = parse_config_args(&["config", "advertise-addr", "host:7100"]).expect("hyphen key");
    assert_eq!(hyphen.key, Some(ConfigKey::AdvertiseAddr));
    assert_eq!(hyphen.value.as_deref(), Some("host:7100"));
}

#[test]
fn apply_set_refuses_to_persist_a_semantically_invalid_document() {
    let mut config = PersistedConfig {
        name: Some("alice".to_string()),
        port: Some(0),
        advertise_addr: None,
    };
    let err = apply_set(&mut config, ConfigKey::Name, "bob").expect_err("invalid sibling key");
    assert!(err.to_string().contains("port 0"), "error: {err}");
}

#[test]
fn render_list_text_only_includes_set_keys() {
    let config = PersistedConfig {
        name: Some("alice".to_string()),
        port: None,
        advertise_addr: Some("host:7100".to_string()),
    };

    let rendered = render_list_text(&config);
    assert!(rendered.contains("name=alice"));
    assert!(rendered.contains("advertise_addr=host:7100"));
    assert!(!rendered.contains("port="));
}
