use std::path::{Path, PathBuf};

/// Resolve an existing regular file beneath a Noted-managed directory.
/// Canonicalizing both paths closes `..`, sibling-prefix, and symlink escapes.
pub(crate) fn resolve_existing_file(
    managed_root: &Path,
    requested: &Path,
    max_bytes: u64,
) -> Result<PathBuf, String> {
    let root = managed_root
        .canonicalize()
        .map_err(|_| "Managed storage is unavailable".to_string())?;
    let resolved = requested
        .canonicalize()
        .map_err(|_| "Managed file could not be found".to_string())?;
    if !resolved.starts_with(&root) {
        return Err("File is outside Noted's managed storage".into());
    }

    let metadata = resolved.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Managed path is not a file".into());
    }
    if metadata.len() > max_bytes {
        return Err("Managed file exceeds the size limit".into());
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "noted-managed-file-test-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn accepts_nested_files_and_rejects_escapes_and_oversize_files() {
        let fixture = fixture_root();
        let root = fixture.join("images");
        let sibling = fixture.join("images-copy");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let nested = root.join("nested/photo.jpg");
        let outside = sibling.join("private.jpg");
        fs::write(&nested, b"ok").unwrap();
        fs::write(&outside, b"secret").unwrap();

        assert_eq!(
            resolve_existing_file(&root, &nested, 2).unwrap(),
            nested.canonicalize().unwrap()
        );
        assert!(resolve_existing_file(&root, &outside, 100).is_err());
        assert!(resolve_existing_file(&root, &nested, 1).is_err());
        assert!(resolve_existing_file(&root, &root.join("nested"), 100).is_err());

        fs::remove_dir_all(&fixture).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_that_escapes_the_managed_root() {
        use std::os::unix::fs::symlink;

        let fixture = fixture_root();
        let root = fixture.join("inbox");
        let outside = fixture.join("outside.jpg");
        let link = root.join("link.jpg");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, b"secret").unwrap();
        symlink(&outside, &link).unwrap();

        assert!(resolve_existing_file(&root, &link, 100).is_err());

        fs::remove_dir_all(&fixture).unwrap();
    }
}
