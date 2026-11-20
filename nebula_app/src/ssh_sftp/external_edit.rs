//! Download remote files to a managed local temp copy for external-app editing,
//! then atomically upload saved changes back through the shared SFTP transaction.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use sha2::{Digest as _, Sha256};

use super::*;

#[derive(Clone, Debug)]
pub(crate) struct ExternalEditFile {
    pub destination: String,
    pub remote_path: String,
    pub canonical_remote_path: String,
    pub name: String,
    pub local_path: PathBuf,
    pub permissions: Option<u32>,
    pub sha256: [u8; 32],
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Stream-hash a local file on disk so large files never load wholly into RAM.
pub(crate) fn file_fingerprint(path: &Path) -> io::Result<([u8; 32], u64, Option<SystemTime>)> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a regular file: {}", path.display()),
        ));
    }
    let mut file = transaction::open_local_read_blocking(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buf)?;
        if count == 0 {
            break;
        }
        hasher.update(&buf[..count]);
        total = total.saturating_add(count as u64);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((digest, total, metadata.modified().ok()))
}

/// Sanitize a remote file name for local temporary storage (especially Windows
/// reserved characters/device names) while keeping the original remote path untouched
/// for uploading changes back to the server.
pub(crate) fn sanitize_local_filename(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control()
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    // Windows rejects trailing dots and spaces in file names.
    while cleaned.ends_with(['.', ' ']) {
        cleaned.pop();
    }
    if cleaned.is_empty() || matches!(cleaned.as_str(), "." | "..") {
        cleaned = "file".to_owned();
    }
    let stem = cleaned.split('.').next().unwrap_or(&cleaned).to_ascii_uppercase();
    let is_reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if is_reserved { format!("_{cleaned}") } else { cleaned }
}

fn detached_context(destination: &str, path: &str) -> TaskContext {
    TaskContext {
        state: Arc::new(Mutex::new(SftpSnapshot {
            destination: destination.to_owned(),
            path: path.to_owned(),
            entries: Vec::new(),
            phase: SftpPhase::Working,
            error: None,
            progress: None,
        })),
        task_control: Arc::new(AtomicUsize::new(0)),
        generation: Arc::new(AtomicU64::new(1)),
        task_generation: 1,
        wake: Arc::new(|| {}),
        last_wake: Arc::new(Mutex::new(Instant::now())),
    }
}

/// Download one remote file into `temp_dir/<name>` and return its baseline fingerprint.
pub(crate) async fn download_for_external_edit(
    destination: &str,
    remote_path: &str,
    temp_dir: &Path,
) -> Result<ExternalEditFile, String> {
    let normalized = normalize_remote_path("/", remote_path);
    let name = normalized
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Invalid remote file path".to_owned())?
        .to_owned();
    validate_name(&name).map_err(str::to_owned)?;

    let sftp = crate::ssh_session::open_sftp(destination)
        .await
        .map_err(|err| format!("Failed to connect to {destination}: {err}"))?;
    let canonical = sftp
        .canonicalize(normalized.clone())
        .await
        .map_err(|err| format!("Failed to resolve {normalized}: {err}"))?;
    let metadata = sftp
        .metadata(canonical.clone())
        .await
        .map_err(|err| format!("Failed to stat {canonical}: {err}"))?;
    if metadata.is_dir() {
        return Err("Cannot open a directory with an external file editor".to_owned());
    }
    tokio::fs::create_dir_all(temp_dir)
        .await
        .map_err(|err| format!("Failed to create temporary directory: {err}"))?;
    let local_name = sanitize_local_filename(&name);
    let local_path = temp_dir.join(&local_name);
    let context = detached_context(destination, &canonical);
    let modified = u64::from(metadata.mtime.unwrap_or(0));
    // `temp_dir` is a brand-new private folder owned by this edit session, so
    // download directly with one synchronously closed OS file handle—avoiding
    // a staging rename right before the external app opens the local copy.
    if let Err(err) = transfer::download_segmented_with_times(
        &sftp,
        &canonical,
        metadata.len(),
        &local_path,
        limits::window_per_file(1),
        modified,
        &context,
    )
    .await
    {
        let _ = transaction::remove_local_file_retry(&local_path).await;
        return Err(err.to_string());
    }

    let fingerprint_path = local_path.clone();
    let (sha256, size, local_modified) =
        tokio::task::spawn_blocking(move || file_fingerprint(&fingerprint_path))
            .await
            .map_err(|err| format!("Failed to fingerprint local copy: {err}"))?
            .map_err(|err| format!("Failed to read local copy: {err}"))?;

    Ok(ExternalEditFile {
        destination: destination.to_owned(),
        remote_path: normalized,
        canonical_remote_path: canonical,
        name,
        local_path,
        permissions: metadata.permissions,
        sha256,
        size,
        modified: local_modified,
    })
}

/// Atomically upload the edited temporary file back to its original remote path,
/// preserving the original remote file permissions when available.
pub(crate) async fn upload_external_edit(
    destination: &str,
    canonical_remote_path: &str,
    local_path: &Path,
    permissions: Option<u32>,
) -> Result<([u8; 32], u64, Option<SystemTime>), String> {
    let read_path = local_path.to_owned();
    let (sha256, size, modified, mut stamp) = tokio::task::spawn_blocking(move || {
        let (sha256, size, modified) = file_fingerprint(&read_path)?;
        let stamp = transaction::FileStamp::read(&read_path)?;
        Ok::<_, io::Error>((sha256, size, modified, stamp))
    })
    .await
    .map_err(|err| format!("Failed to inspect local file: {err}"))?
    .map_err(|err| format!("Failed to read local file: {err}"))?;

    if permissions.is_some() {
        stamp.permissions = permissions;
    }

    let sftp = crate::ssh_session::open_sftp(destination)
        .await
        .map_err(|err| format!("Failed to connect to {destination}: {err}"))?;
    let context = detached_context(destination, canonical_remote_path);
    transaction::upload_file(&sftp, local_path, canonical_remote_path, stamp, false, &context)
        .await
        .map_err(|err| err.to_string())?;

    Ok((sha256, size, modified))
}

#[cfg(test)]
mod tests {
    use super::{file_fingerprint, sanitize_local_filename};

    #[test]
    fn fingerprint_detects_same_length_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"port = 8080\n").unwrap();
        let (first_hash, first_size, _) = file_fingerprint(&path).unwrap();

        std::fs::write(&path, b"port = 9090\n").unwrap();
        let (second_hash, second_size, _) = file_fingerprint(&path).unwrap();

        assert_eq!(first_size, second_size);
        assert_ne!(first_hash, second_hash);
    }

    #[test]
    fn sanitize_local_filename_handles_windows_illegal_chars_and_reserved_names() {
        assert_eq!(sanitize_local_filename("app.conf"), "app.conf");
        assert_eq!(sanitize_local_filename("2026-11-20_12:00:00.log"), "2026-11-20_12_00_00.log");
        assert_eq!(sanitize_local_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_local_filename("a<b>c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_local_filename("trailing. "), "trailing");
    }
}
