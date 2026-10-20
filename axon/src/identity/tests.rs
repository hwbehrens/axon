use super::*;
use crate::config::AxonPaths;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn derive_agent_id_is_40_chars_with_prefix() {
    let mut seed = [7u8; 32];
    let key = SigningKey::from_bytes(&seed);
    let id = derive_agent_id(&key.verifying_key());
    assert_eq!(id.len(), 40);
    assert!(id.starts_with("ed25519."));
    assert!(id[8..].chars().all(|c| c.is_ascii_hexdigit()));

    seed[0] = 8;
    let other = SigningKey::from_bytes(&seed);
    assert_ne!(id, derive_agent_id(&other.verifying_key()));
}

#[test]
fn derive_agent_id_is_deterministic() {
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let id1 = derive_agent_id(&key.verifying_key());
    let id2 = derive_agent_id(&key.verifying_key());
    assert_eq!(id1, id2);
}

#[test]
fn load_or_generate_roundtrip_persists_identity() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));

    let first = Identity::load_or_generate(&paths).expect("first load");
    let second = Identity::load_or_generate(&paths).expect("second load");

    assert_eq!(first.agent_id(), second.agent_id());
    assert_eq!(first.public_key_base64(), second.public_key_base64());
}

#[test]
fn private_key_file_permissions() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    Identity::load_or_generate(&paths).expect("generate");

    let mode = fs::metadata(&paths.identity_key)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn existing_private_key_permissions_are_normalized() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let seed = [9u8; 32];
    fs::write(&paths.identity_key, STANDARD.encode(seed)).expect("write key");
    fs::set_permissions(&paths.identity_key, fs::Permissions::from_mode(0o644))
        .expect("set permissive mode");

    Identity::load_or_generate(&paths).expect("load existing key");

    let mode = fs::metadata(&paths.identity_key)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn public_key_file_is_valid_base64() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("generate");

    let pub_b64 = fs::read_to_string(&paths.identity_pub).expect("read pub");
    let decoded = STANDARD.decode(pub_b64.trim()).expect("decode");
    assert_eq!(decoded.len(), 32);
    assert_eq!(decoded, identity.verifying_key().to_bytes());
}

#[test]
fn cert_generation_produces_material() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let identity = Identity::load_or_generate(&paths).expect("generate");
    let cert = identity.make_quic_certificate().expect("cert");

    assert!(!cert.cert_der.is_empty());
    assert!(!cert.key_der.is_empty());
}

#[test]
fn invalid_key_length_is_rejected() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let bad_b64 = STANDARD.encode([1u8; 16]);
    fs::write(&paths.identity_key, &bad_b64).expect("write bad key");

    let result = Identity::load_or_generate(&paths);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("expected 32 decoded bytes"));
}

#[test]
fn legacy_raw_key_is_rejected() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let raw_seed = [7u8; 32];
    fs::write(&paths.identity_key, raw_seed).expect("write raw key");

    let result = Identity::load_or_generate(&paths);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("invalid identity.key"));
}

#[test]
fn private_key_is_created_at_0600_before_any_bytes_exist() {
    // A test that only inspects the final mode cannot catch the
    // write-then-chmod window: the file must be restrictive from the moment
    // it exists (mode applies at open(2)), while it is still empty.
    let dir = tempdir().expect("tempdir");
    let path = PathBuf::from(dir.path()).join("identity.key");

    let file = create_private_key_file(&path).expect("create key file");
    let meta = file.metadata().expect("metadata");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.len(), 0, "the file must be observed before content");
}

#[test]
fn private_key_creation_refuses_to_clobber_an_existing_key() {
    let dir = tempdir().expect("tempdir");
    let path = PathBuf::from(dir.path()).join("identity.key");
    fs::write(&path, "preexisting").expect("seed file");

    let err = write_seed_as_base64(&path, &[1u8; 32]).expect_err("clobber must fail");
    assert!(
        err.to_string().contains("failed to write private key"),
        "unexpected error: {err:#}"
    );
    assert_eq!(fs::read_to_string(&path).expect("read"), "preexisting");
}

#[test]
fn load_refuses_symlinked_identity_key() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let target = PathBuf::from(dir.path()).join("real-identity.key");
    fs::write(&target, STANDARD.encode([3u8; 32])).expect("seed target");
    std::os::unix::fs::symlink(&target, &paths.identity_key).expect("symlink");

    let err = Identity::load_or_generate(&paths).expect_err("symlink must be refused");
    assert!(
        err.to_string().contains("non-regular identity.key"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn load_refuses_fifo_identity_key_without_blocking() {
    use std::time::Duration;

    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");
    let fifo_c =
        std::ffi::CString::new(paths.identity_key.to_str().expect("utf8 path")).expect("cstring");
    assert_eq!(
        unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) },
        0,
        "mkfifo fixture"
    );

    // Regression shape: an identity.key FIFO must be refused by the type
    // check, never opened (an open with no writer would block forever). The
    // timeout converts that hang into a test failure.
    let probe_paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Identity::load_or_generate(&probe_paths).map(|_| ()));
    });
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("load must not hang on a FIFO identity.key");
    let err = result.expect_err("FIFO identity.key must be refused");
    assert!(
        err.to_string().contains("non-regular identity.key"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn load_refuses_oversized_identity_key_before_decoding() {
    let dir = tempdir().expect("tempdir");
    let paths = AxonPaths::from_root(PathBuf::from(dir.path()));
    paths.ensure_root_exists().expect("ensure root");

    let oversized = "A".repeat(MAX_IDENTITY_KEY_BYTES + 1);
    fs::write(&paths.identity_key, oversized).expect("write oversized key");

    let err = Identity::load_or_generate(&paths).expect_err("oversized key must be refused");
    assert!(
        err.to_string().contains("exceeds the"),
        "oversized input must be refused before decoding, got: {err:#}"
    );
}
