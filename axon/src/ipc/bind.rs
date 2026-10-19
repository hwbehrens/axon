//! Private-mode Unix socket binding.
//!
//! Spec: the daemon creates the socket with mode `0600`
//! (spec/IPC.md §2). Binding directly at the final path and chmodding
//! afterwards leaves a window where the socket carries a umask-derived mode
//! (commonly `0755`), during which another local user could connect. The
//! socket is instead created inside a private staging directory, chmodded
//! there, and renamed into place: the final path never exists with a mode
//! other than `0600`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use tokio::net::UnixListener;

/// Bind a Unix listener at `socket_path` with final mode `0600`, creating
/// missing parent directories. The listener stays valid after the socket is
/// renamed into place (the owning inode does not move).
pub(crate) fn bind_socket(socket_path: &Path) -> Result<UnixListener> {
    let parent = match socket_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("failed to create socket dir: {}", parent.display()))?;

    // The staging name only has to be unique and short: `sockaddr_un.sun_path`
    // is 108 bytes on Linux and 104 on macOS (NUL included), so a verbose
    // name would reject otherwise-valid long socket paths.
    let staging = parent.join(format!(".s{:08x}", uuid::Uuid::new_v4().as_u128() as u32));
    let staged = staging.join("s");
    if staged.as_os_str().len() > max_socket_path_bytes() {
        // The fallback keeps the historical bind-then-chmod order for paths
        // where a private staging directory cannot fit; refusing to bind a
        // previously valid path would be worse than the brief window.
        return bind_and_restrict(socket_path);
    }

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(&staging)
        .with_context(|| format!("failed to create socket staging dir: {}", staging.display()))?;

    let result = (|| -> Result<UnixListener> {
        let listener = UnixListener::bind(&staged)
            .with_context(|| format!("failed to bind unix socket: {}", socket_path.display()))?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600)).with_context(
            || {
                format!(
                    "failed to set socket permissions: {}",
                    socket_path.display()
                )
            },
        )?;
        std::fs::rename(&staged, socket_path)
            .with_context(|| format!("failed to publish unix socket: {}", socket_path.display()))?;
        Ok(listener)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    // On every path the staging dir is empty here (the socket was renamed
    // out or removed above), so removal is best-effort cleanup only.
    let _ = std::fs::remove_dir(&staging);
    result
}

/// Bind directly at the final path, then restrict it. Used only when the
/// private staging path cannot fit the platform's `sun_path` bound.
fn bind_and_restrict(socket_path: &Path) -> Result<UnixListener> {
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind unix socket: {}", socket_path.display()))?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600)).with_context(
        || {
            format!(
                "failed to set socket permissions: {}",
                socket_path.display()
            )
        },
    )?;
    Ok(listener)
}

/// Platform `sun_path` capacity excluding the NUL terminator.
fn max_socket_path_bytes() -> usize {
    std::mem::size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path) - 1
}

#[cfg(test)]
#[path = "bind_tests.rs"]
mod tests;
