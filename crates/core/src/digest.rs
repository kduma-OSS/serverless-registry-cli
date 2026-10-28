//! File digests used when declaring an upload to the registry.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::Result;

/// Computes the SHA-256 digest of a file.
///
/// Returns the lower-case hex digest and the file size in bytes. The file is read in
/// chunks, so large archives are not loaded into memory.
pub fn sha256_file(path: impl AsRef<Path>) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;

    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        hasher.update(&buffer[..read]);
        size += read as u64;
    }

    Ok((hex::encode(hasher.finalize()), size))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn hashes_file_contents_and_reports_size() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"hello").unwrap();

        let (digest, size) = sha256_file(file.path()).unwrap();

        assert_eq!(
            digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(size, 5);
    }

    #[test]
    fn hashes_empty_file() {
        let file = tempfile::NamedTempFile::new().unwrap();

        let (digest, size) = sha256_file(file.path()).unwrap();

        assert_eq!(
            digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(size, 0);
    }

    #[test]
    fn reports_missing_file_as_io_error() {
        let dir = tempfile::tempdir().unwrap();

        let err = sha256_file(dir.path().join("missing")).unwrap_err();

        assert!(matches!(err, crate::Error::Io(_)));
    }
}
