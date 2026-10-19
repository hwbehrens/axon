//! Socket-binding tests: the final path only ever exists with mode 0600 and
//! no staging artifacts survive a bind (I6 nit: bind-then-chmod window).

use std::os::unix::fs::PermissionsExt;

use super::{STAGING_DIR_PREFIX, bind_and_restrict, bind_socket};

fn socket_mode(path: &std::path::Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777
}

/// Leftover artifacts of `bind_socket`'s private staging directory. The
/// filter must match the real producer name (`.s<8 hex>`), not a
/// never-created prefix, or the "no staging artifacts survive" checks below
/// are vacuous.
fn staging_leftovers(dir: &std::path::Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(dir)
        .expect("read socket dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| {
            let Some(name) = name.to_str() else {
                return false;
            };
            let Some(hex) = name.strip_prefix(STAGING_DIR_PREFIX) else {
                return false;
            };
            hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit())
        })
        .collect()
}

#[test]
fn staging_leftover_guard_matches_the_producers_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let staged = format!("{STAGING_DIR_PREFIX}deadbeef");
    std::fs::create_dir(dir.path().join(&staged)).expect("create leftover staging dir");

    assert_eq!(
        staging_leftovers(dir.path()),
        vec![std::ffi::OsString::from(staged)],
        "the cleanup guard must observe a real leftover staging directory"
    );
}

#[tokio::test]
async fn bind_publishes_the_socket_with_mode_0600_and_no_staging() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("axon.sock");

    let _listener = bind_socket(&socket).expect("bind");

    let mode = std::fs::symlink_metadata(&socket)
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the published socket must be owner-only");
    assert!(
        staging_leftovers(dir.path()).is_empty(),
        "a completed bind must leave no staging directory"
    );
}

#[tokio::test]
async fn bind_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("nested/deeper/axon.sock");

    let _listener = bind_socket(&socket).expect("bind creates parents");

    assert!(socket.exists(), "the socket must be published");
    assert!(staging_leftovers(socket.parent().expect("parent")).is_empty());
}

/// The fallback used when a private staging dir cannot fit the platform's
/// `sun_path` bound must still publish an owner-only socket.
#[tokio::test]
async fn fallback_direct_bind_still_restricts_the_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("axon.sock");

    let _listener = bind_and_restrict(&socket).expect("direct bind");

    assert_eq!(socket_mode(&socket), 0o600);
}
