use std::path::Path;

use anyhow::{Context, Result};
use tracing::{info, warn};

/// Load the IPC token from disk, generating a new one if it doesn't exist.
/// Returns the token string (64 hex chars) on success.
pub async fn load_or_generate(token_path: &Path) -> Result<Option<String>> {
    if !token_path.exists() {
        generate_token_file(token_path).await.map(Some)
    } else {
        validate_and_read(token_path).await.map(Some)
    }
}

/// Re-read the IPC token file from disk. Used on SIGHUP for token rotation.
pub async fn reload(token_path: &Path) -> Result<String> {
    read_with_nofollow(token_path).await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async fn generate_token_file(token_path: &Path) -> Result<String> {
    // Generate a random 256-bit token (64 hex chars)
    let mut token_bytes = [0u8; 32];
    getrandom::getrandom(&mut token_bytes).context("failed to generate random token")?;
    let token = hex::encode(token_bytes);

    // Atomic write: write to temp file then rename (IPC.md §2.2)
    // Use randomized temp name to prevent symlink attacks on predictable paths
    let mut tmp_name_bytes = [0u8; 8];
    getrandom::getrandom(&mut tmp_name_bytes)
        .context("failed to generate random temp filename")?;
    let tmp_name = format!(".ipc-token.{}.tmp", hex::encode(tmp_name_bytes));
    let tmp_path = token_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(tmp_name);

    // Ensure parent directory exists (supports custom token paths like ~/.axon/agentA/ipc-token)
    if let Some(parent) = token_path.parent()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create token parent directory: {}", parent.display()))?;
    }

    // Check for existing symlink at temp path (security: prevent symlink attacks)
    if tmp_path.exists() {
        let tmp_meta = tokio::fs::symlink_metadata(&tmp_path)
            .await
            .with_context(|| {
                format!(
                    "failed to read metadata for temp token path: {}",
                    tmp_path.display()
                )
            })?;
        if tmp_meta.file_type().is_symlink() {
            anyhow::bail!(
                "IPC token temp path is a symlink (security violation): {}. \
                 Remove it and restart the daemon.",
                tmp_path.display()
            );
        }
        // Remove stale non-symlink temp file
        tokio::fs::remove_file(&tmp_path).await.with_context(|| {
            format!("failed to remove stale temp file: {}", tmp_path.display())
        })?;
    }

    // Create with O_CREAT|O_EXCL semantics (create_new) and restrictive permissions
    let token_clone = token.clone();
    let tmp_path_clone = tmp_path.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path_clone)
            .with_context(|| {
                format!(
                    "failed to create IPC token temp file: {}",
                    tmp_path_clone.display()
                )
            })?;
        file.write_all(token_clone.as_bytes())
            .context("failed to write IPC token to temp file")?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("token write task panicked")??;

    let token_path_owned = token_path.to_path_buf();
    tokio::fs::rename(&tmp_path, &token_path_owned)
        .await
        .with_context(|| {
            format!(
                "failed to rename IPC token temp file from {} to {}",
                tmp_path.display(),
                token_path_owned.display()
            )
        })?;

    info!(path = %token_path.display(), "generated new IPC token");
    Ok(token)
}

async fn validate_and_read(token_path: &Path) -> Result<String> {
    // Validate existing token file (IPC.md §2.2):
    // Must not be a symlink, must be a regular file, must be owned by us
    let meta = tokio::fs::symlink_metadata(token_path)
        .await
        .with_context(|| {
            format!(
                "failed to read metadata for IPC token: {}",
                token_path.display()
            )
        })?;

    if meta.file_type().is_symlink() {
        anyhow::bail!(
            "IPC token file is a symlink (security violation): {}. \
             Remove it and restart the daemon.",
            token_path.display()
        );
    }

    if !meta.file_type().is_file() {
        anyhow::bail!(
            "IPC token path is not a regular file: {}. \
             Remove it and restart the daemon.",
            token_path.display()
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let owner_uid = meta.uid();
        let my_uid = unsafe { libc::getuid() };
        if owner_uid != my_uid {
            anyhow::bail!(
                "IPC token file is owned by UID {} but daemon runs as UID {} \
                 (security violation): {}. Remove it and restart the daemon.",
                owner_uid,
                my_uid,
                token_path.display()
            );
        }
        let mode = meta.mode() & 0o777;
        if mode != 0o600 {
            warn!(
                path = %token_path.display(),
                mode = format!("{:o}", mode),
                "IPC token file has unexpected permissions (expected 0600), fixing"
            );
            tokio::fs::set_permissions(token_path, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| {
                    format!(
                        "failed to fix permissions on IPC token file: {}",
                        token_path.display()
                    )
                })?;
        }
    }

    read_with_nofollow(token_path).await
}

async fn read_with_nofollow(token_path: &Path) -> Result<String> {
    let path = token_path.to_path_buf();
    let token = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;

        let meta = std::fs::symlink_metadata(&path).with_context(|| {
            format!("failed to read metadata for IPC token: {}", path.display())
        })?;
        if meta.file_type().is_symlink() {
            anyhow::bail!(
                "IPC token file is a symlink (security violation): {}",
                path.display()
            );
        }
        if !meta.file_type().is_file() {
            anyhow::bail!(
                "IPC token path is not a regular file: {}",
                path.display()
            );
        }

        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| {
                format!("failed to open IPC token (O_NOFOLLOW): {}", path.display())
            })?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .context("failed to read IPC token file")?;
        Ok(contents.trim().to_string())
    })
    .await
    .context("token read task panicked")??;
    Ok(token)
}
