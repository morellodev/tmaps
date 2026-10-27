use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Readers see the old or the new file, never a part. A symlink stays a symlink, and the file
/// keeps its permissions.
///
/// # Errors
///
/// Returns the error if the directory or the file cannot be written.
pub fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let existing = fs::canonicalize(path).ok();
    let path = existing.as_deref().unwrap_or(path);
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    if existing.is_some() {
        temp.as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
    }
    temp.write_all(contents)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn write_replaces_the_file_and_leaves_no_temporary_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new").join("view.toml");
        write_atomically(&path, b"old").unwrap();
        write_atomically(&path, b"new").unwrap();
        let files = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!((fs::read(&path).unwrap(), files), (b"new".to_vec(), 1));
    }

    #[cfg(unix)]
    #[test]
    fn write_through_a_symlink_keeps_the_link_and_the_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = TempDir::new().unwrap();
        let target = dir.path().join("dotfiles.toml");
        let link = dir.path().join("config.toml");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&target, &link).unwrap();
        write_atomically(&link, b"new").unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            (
                fs::symlink_metadata(&link).unwrap().is_symlink(),
                fs::read(&target).unwrap(),
                mode
            ),
            (true, b"new".to_vec(), 0o644)
        );
    }
}
