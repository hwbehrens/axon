use super::*;

#[test]
fn roundtrip_encodes_and_decodes() {
    let pubkey = STANDARD.encode([7u8; 32]);
    let token = encode(&pubkey, "127.0.0.1:7100").expect("encode");
    assert!(token.starts_with("axon://"));

    let decoded = decode(&token).expect("decode");
    assert_eq!(decoded.pubkey, pubkey);
    assert_eq!(decoded.addr, "127.0.0.1:7100");
    assert!(decoded.agent_id.as_str().starts_with("ed25519."));
}

#[test]
fn decode_rejects_bad_scheme() {
    let err = decode("http://abc@127.0.0.1:7100").expect_err("bad scheme should fail");
    assert!(err.to_string().contains("must start"));
}

#[test]
fn decode_rejects_invalid_pubkey_b64url() {
    let err = decode("axon://@@127.0.0.1:7100").expect_err("bad pubkey should fail");
    assert!(err.to_string().contains("empty"));
}

#[test]
fn decode_rejects_invalid_pubkey_length() {
    let short = URL_SAFE_NO_PAD.encode([1u8; 8]);
    let err =
        decode(&format!("axon://{short}@127.0.0.1:7100")).expect_err("short pubkey should fail");
    assert!(err.to_string().contains("32 bytes"));
}

#[test]
fn encode_rejects_malformed_addr() {
    let pubkey = STANDARD.encode([7u8; 32]);
    let err = encode(&pubkey, "host-without-port").expect_err("bad addr should fail");
    assert!(err.to_string().contains("host:port"));
}

#[test]
fn derive_agent_id_from_base64_rejects_wrong_length() {
    let pubkey = STANDARD.encode([1u8; 8]);
    let err = derive_agent_id_from_pubkey_base64(&pubkey).expect_err("short key");
    assert!(err.to_string().contains("32 bytes"));
}

#[test]
fn addr_validation_matches_peer_locator_parse() {
    // One authority for the address segment: anything the enrollment path
    // (`PeerLocator::parse`) rejects must be rejected by the token codec
    // too, in both directions.
    let pubkey = STANDARD.encode([7u8; 32]);
    for addr in [
        "127.0.0.1:7100",
        "peer.local:7100",
        "[::1]:7100",
        "peer name:7100",
        "peer.local:0",
        "peer.local:65536",
        "peer.local",
        ":7100",
        "peer.local:",
    ] {
        assert_eq!(
            encode(&pubkey, addr).is_ok(),
            PeerLocator::parse(addr).is_ok(),
            "token encode and PeerLocator::parse disagree about {addr:?}"
        );
    }
}

#[test]
fn decode_rejects_addr_that_peer_locator_rejects() {
    let key = URL_SAFE_NO_PAD.encode([7u8; 32]);
    for addr in ["peer name:7100", "peer.local:0"] {
        assert!(
            PeerLocator::parse(addr).is_err(),
            "test input {addr:?} must be locator-invalid"
        );
        let err = decode(&format!("axon://{key}@{addr}"))
            .expect_err("locator-invalid addr must fail token decode");
        assert!(
            err.to_string().contains("addr"),
            "unexpected error for {addr:?}: {err:#}"
        );
    }
}

#[test]
fn locator_valid_addrs_round_trip_through_decode() {
    let pubkey = STANDARD.encode([7u8; 32]);
    for addr in ["127.0.0.1:7100", "peer.local:7100", "[::1]:7100"] {
        let token = encode(&pubkey, addr).expect("locator-valid addr must encode");
        let decoded = decode(&token).expect("locator-valid token must decode");
        assert_eq!(decoded.addr, addr);
        assert_eq!(decoded.pubkey, pubkey);
    }
}
