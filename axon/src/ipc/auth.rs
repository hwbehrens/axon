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
