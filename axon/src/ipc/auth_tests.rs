use super::*;
use tokio::net::UnixListener;

#[tokio::test]
async fn peer_uid_returns_current_user() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("test.sock");

    let listener = UnixListener::bind(&socket_path).expect("bind");

    let client_task =
        tokio::spawn(async move { UnixStream::connect(&socket_path).await.expect("connect") });

    let (server_stream, _) = listener.accept().await.expect("accept");
    let _client_stream = client_task.await.expect("client task");

    let uid = peer_uid(&server_stream);

    // On supported platforms, should return the current UID
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        assert!(uid.is_some());
        let expected_uid = unsafe { libc::getuid() };
        assert_eq!(uid.unwrap(), expected_uid);
    }

    // On unsupported platforms, returns None
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        assert!(uid.is_none());
    }
}
