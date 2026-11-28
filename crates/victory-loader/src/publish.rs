//! Portable publication of derived files into a shared cache directory.
//!
//! Several copies of the runner can prepare the same title at once. Each
//! writer stages into its own file, so two of them cannot truncate each
//! other's bytes or rename a file the other already published. The final
//! rename is atomic within the directory, and the staging file is removed
//! if publication fails.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` unless it is already there, using a staging file
/// unique to this process and call.
pub fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image");
    let sequence = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
    let staging = path.with_file_name(format!(
        ".{name}.{}.{sequence}.tmp",
        std::process::id()
    ));
    std::fs::write(&staging, bytes)?;
    let mut renamed = std::fs::rename(&staging, path);
    // Replacing a file that another publisher has just replaced, or is reading to see whether it is
    // there already, can be refused for that moment (access denied, a sharing violation: the old
    // file is open without the right to be replaced). The publisher whose name stays is not owed an
    // error: once the file holds these bytes, it is published, whoever did it.
    for _ in 0..50 {
        let Err(error) = &renamed else { break };
        if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
            renamed = Ok(());
            break;
        }
        if !matches!(error.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        renamed = std::fs::rename(&staging, path);
    }
    if let Err(error) = renamed {
        let _ = std::fs::remove_file(&staging);
        return Err(error);
    }
    // (If another publisher's file won, ours is still there: gone.)
    let _ = std::fs::remove_file(&staging);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two publishers staging the same destination at once both succeed,
    /// and neither leaves a staging file behind.
    #[test]
    fn concurrent_publishers_do_not_share_a_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.bin");
        let bytes = b"a prepared image".to_vec();
        let (first, second) = (path.clone(), path.clone());
        let (first_bytes, second_bytes) = (bytes.clone(), bytes.clone());
        let (one, two) = (
            std::thread::spawn(move || write_if_changed(&first, &first_bytes)),
            std::thread::spawn(move || write_if_changed(&second, &second_bytes)),
        );
        one.join().unwrap().unwrap();
        two.join().unwrap().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path() != path)
            .collect();
        assert!(leftovers.is_empty(), "staging files were removed: {leftovers:?}");
    }

    #[test]
    fn a_failed_publication_removes_its_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-parent").join("image.bin");
        assert!(write_if_changed(&path, b"bytes").is_err());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .collect();
        assert!(leftovers.is_empty(), "no staging file survives an error");
    }
}
