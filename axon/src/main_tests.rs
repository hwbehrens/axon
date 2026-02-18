use super::*;
use clap::error::ErrorKind;

#[test]
fn parse_agent_id_arg_normalizes_case() {
    let parsed = parse_agent_id_arg("ED25519.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        .expect("mixed/upper case should parse");
    assert_eq!(parsed, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
}

#[test]
fn cli_verbose_flag_parses_short_and_long() {
    let long = Cli::try_parse_from(["axon", "--verbose", "status"]).expect("parse --verbose");
    assert!(long.verbose);
    assert!(matches!(long.command, Commands::Status { .. }));

    let short = Cli::try_parse_from(["axon", "-v", "status"]).expect("parse -v");
    assert!(short.verbose);
    assert!(matches!(short.command, Commands::Status { .. }));

    let default = Cli::try_parse_from(["axon", "status"]).expect("parse without verbose");
    assert!(!default.verbose);
    assert!(matches!(default.command, Commands::Status { .. }));
}

#[test]
fn doctor_rekey_requires_fix_flag() {
    let err = Cli::try_parse_from(["axon", "doctor", "--rekey"])
        .expect_err("--rekey without --fix should fail");
    assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
}

#[test]
fn identity_flags_parse_json_and_addr() {
    let cli = Cli::try_parse_from(["axon", "identity", "--json", "--addr", "10.0.0.7:7100"])
        .expect("parse identity flags");
    match cli.command {
        Commands::Identity { json, addr } => {
            assert!(json);
            assert_eq!(addr.as_deref(), Some("10.0.0.7:7100"));
        }
        _ => panic!("expected identity command"),
    }
}

#[test]
fn connect_command_parses_token() {
    let cli = Cli::try_parse_from(["axon", "connect", "axon://abc@127.0.0.1:7100"])
        .expect("parse connect");
    match cli.command {
        Commands::Connect { token } => assert_eq!(token, "axon://abc@127.0.0.1:7100"),
        _ => panic!("expected connect command"),
    }
}

#[test]
fn request_command_parses_timeout() {
    let cli = Cli::try_parse_from([
        "axon",
        "request",
        "--timeout",
        "9",
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "hello",
    ])
    .expect("parse request");

    match cli.command {
        Commands::Request {
            agent_id,
            timeout,
            message,
        } => {
            assert_eq!(agent_id, "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            assert_eq!(timeout, 9);
            assert_eq!(message, "hello");
        }
        _ => panic!("expected request command"),
    }
}

#[test]
fn send_subcommand_is_not_accepted() {
    let err = Cli::try_parse_from([
        "axon",
        "send",
        "ed25519.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "hello",
    ])
    .expect_err("send should be unknown");
    assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
}

#[test]
fn select_identity_addr_prefers_override_then_config() {
    let override_addr =
        select_identity_addr(Some("10.0.0.1:7100"), Some("ignored:7200"), 7300).expect("override");
    assert_eq!(override_addr, "10.0.0.1:7100");

    let config_addr =
        select_identity_addr(None, Some("alice.tailnet:7100"), 7300).expect("config advertise");
    assert_eq!(config_addr, "alice.tailnet:7100");
}
