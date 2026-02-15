use tokio::net::UnixStream;

/// Extract the peer UID from a Unix domain socket connection.
/// Returns Some(uid) if available, None otherwise.
#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut ucred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut ucred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };

    if result == 0 { Some(ucred.uid) } else { None }
}

#[cfg(target_os = "macos")]
pub fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;

    let result = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };

    if result == 0 { Some(uid) } else { None }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn peer_uid(_stream: &UnixStream) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
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
}
