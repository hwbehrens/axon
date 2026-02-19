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
