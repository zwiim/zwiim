//! Zwiim-owned: the files a playbook's inputs use, kept by the Host
//! (recording, design 2026-11-22 §4).
//!
//! A file is copied in when the person adds or replaces it, so moving or
//! deleting the original never breaks a playbook. Each copy lives at
//! `<data>/playbook-files/<sha256>/<file name>`: by content, so a replace
//! keeps the old bytes while a saved version or an item record still names
//! them, and under its own name, so the page it is attached to sees
//! `resume.pdf`, not a hash. Only the person's user can read them.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The largest file an input may be: 50 MB, any type.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// One stored copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub sha256: String,
    pub file: String,
    pub size: u64,
}

#[derive(Debug)]
pub enum StoreError {
    /// Over [`MAX_FILE_BYTES`].
    TooLarge,
    /// Not a regular file, or not there.
    NotAFile,
    Io(io::Error),
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// The Host's store of playbook input files.
#[derive(Debug, Clone)]
pub struct InputFileStore {
    root: PathBuf,
}

impl InputFileStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("playbook-files"),
        }
    }

    /// Copy `source` in, hashing it on the way; an identical copy already
    /// there is reused.
    pub fn put(&self, source: &Path) -> Result<StoredFile, StoreError> {
        let meta = std::fs::metadata(source).map_err(|_| StoreError::NotAFile)?;
        if !meta.is_file() {
            return Err(StoreError::NotAFile);
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(StoreError::TooLarge);
        }
        let file = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .ok_or(StoreError::NotAFile)?;
        make_private_dir(&self.root)?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.root)?;
        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let mut reader = std::fs::File::open(source)?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            // The file may grow after the size check.
            if size > MAX_FILE_BYTES {
                return Err(StoreError::TooLarge);
            }
            hasher.update(&buf[..n]);
            temp.write_all(&buf[..n])?;
        }
        temp.flush()?;
        let sha256 = format!("{:x}", hasher.finalize());
        let dir = self.root.join(&sha256);
        make_private_dir(&dir)?;
        let dest = dir.join(&file);
        if !dest.is_file() {
            set_mode(temp.path(), 0o600)?;
            temp.persist(&dest).map_err(|e| StoreError::Io(e.error))?;
        }
        Ok(StoredFile { sha256, file, size })
    }

    /// Where the copy `file` with content `sha256` lives.
    pub fn path_of(&self, sha256: &str, file: &str) -> PathBuf {
        self.root.join(sha256).join(file)
    }

    /// Delete every copy whose hash is not in `keep`; how many went.
    pub fn remove_unreferenced(&self, keep: &HashSet<String>) -> io::Result<usize> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut removed = 0;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type()?.is_dir() || !is_sha256(&name) || keep.contains(&name) {
                continue;
            }
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
        Ok(removed)
    }
}

/// 64 lowercase hex characters.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn make_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_put_file_keeps_its_name_under_its_hash() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = InputFileStore::new(data.path());
        let stored = store
            .put(&write(src.path(), "resume.pdf", b"my resume"))
            .unwrap();
        assert_eq!(stored.file, "resume.pdf");
        assert_eq!(stored.size, 9);
        assert!(is_sha256(&stored.sha256));
        let path = store.path_of(&stored.sha256, "resume.pdf");
        assert_eq!(
            path,
            data.path()
                .join("playbook-files")
                .join(&stored.sha256)
                .join("resume.pdf")
        );
        assert_eq!(std::fs::read(path).unwrap(), b"my resume");
    }

    #[test]
    fn the_copy_survives_the_original_being_deleted() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = InputFileStore::new(data.path());
        let original = write(src.path(), "resume.pdf", b"keep me");
        let stored = store.put(&original).unwrap();
        std::fs::remove_file(&original).unwrap();
        assert!(store.path_of(&stored.sha256, &stored.file).is_file());
    }

    #[test]
    fn a_file_over_fifty_megabytes_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let path = src.path().join("big.zip");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        let store = InputFileStore::new(data.path());
        assert!(matches!(store.put(&path), Err(StoreError::TooLarge)));
        let exact = src.path().join("exact.bin");
        std::fs::File::create(&exact)
            .unwrap()
            .set_len(MAX_FILE_BYTES)
            .unwrap();
        assert!(store.put(&exact).is_ok());
    }

    #[test]
    fn a_missing_file_or_a_folder_is_not_a_file() {
        let data = tempfile::tempdir().unwrap();
        let store = InputFileStore::new(data.path());
        assert!(matches!(
            store.put(&data.path().join("nope.pdf")),
            Err(StoreError::NotAFile)
        ));
        assert!(matches!(store.put(data.path()), Err(StoreError::NotAFile)));
    }

    #[cfg(unix)]
    #[test]
    fn stored_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = InputFileStore::new(data.path());
        let stored = store.put(&write(src.path(), "a.txt", b"a")).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data.path().join("playbook-files")), 0o700);
        assert_eq!(
            mode(&data.path().join("playbook-files").join(&stored.sha256)),
            0o700
        );
        assert_eq!(mode(&store.path_of(&stored.sha256, "a.txt")), 0o600);
    }

    #[test]
    fn same_name_new_bytes_is_a_new_entry_and_the_old_stays() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = InputFileStore::new(data.path());
        let v1 = store
            .put(&write(src.path(), "resume.pdf", b"version one"))
            .unwrap();
        let v2 = store
            .put(&write(src.path(), "resume.pdf", b"version two"))
            .unwrap();
        assert_ne!(v1.sha256, v2.sha256);
        assert!(store.path_of(&v1.sha256, "resume.pdf").is_file());
        assert!(store.path_of(&v2.sha256, "resume.pdf").is_file());
        let again = store
            .put(&write(src.path(), "resume.pdf", b"version two"))
            .unwrap();
        assert_eq!(again, v2);
    }

    #[test]
    fn unreferenced_copies_are_removed() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = InputFileStore::new(data.path());
        let keep = store.put(&write(src.path(), "a.txt", b"a")).unwrap();
        let gone = store.put(&write(src.path(), "b.txt", b"b")).unwrap();
        let removed = store
            .remove_unreferenced(&HashSet::from([keep.sha256.clone()]))
            .unwrap();
        assert_eq!(removed, 1);
        assert!(store.path_of(&keep.sha256, "a.txt").is_file());
        assert!(!store.path_of(&gone.sha256, "b.txt").exists());
    }
}
