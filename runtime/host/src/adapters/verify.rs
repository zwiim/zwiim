//! Checks every downloaded artifact must pass before it is extracted.

use std::io::Read;
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256, Sha512};

use super::lock::{PackageLock, RegistryKey};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("{0} does not match its locked hash")]
    Hash(String),
    #[error("{0} is not signed by the npm registry")]
    Signature(String),
    #[error("{0}")]
    Malformed(String),
}

/// SHA-256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    stream(path, |chunk| hasher.update(chunk))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// The file's SHA-256 is `expected` (hex).
pub fn sha256(path: &Path, expected: &str, what: &str) -> Result<(), VerifyError> {
    let actual = sha256_file(path).map_err(|e| VerifyError::Malformed(format!("{what}: {e}")))?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(VerifyError::Hash(what.to_string()))
    }
}

/// The file matches a `sha512-<base64>` integrity string.
pub fn integrity(path: &Path, sri: &str, what: &str) -> Result<(), VerifyError> {
    let expected = sri
        .strip_prefix("sha512-")
        .and_then(|b64| STANDARD.decode(b64).ok())
        .ok_or_else(|| VerifyError::Malformed(format!("{what}: unsupported integrity {sri}")))?;
    let mut hasher = Sha512::new();
    stream(path, |chunk| hasher.update(chunk))
        .map_err(|e| VerifyError::Malformed(format!("{what}: {e}")))?;
    if hasher.finalize().as_slice() == expected.as_slice() {
        Ok(())
    } else {
        Err(VerifyError::Hash(what.to_string()))
    }
}

/// The registry signed this package's name, version and integrity with one
/// of the locked keys.
pub fn registry_signature(keys: &[RegistryKey], package: &PackageLock) -> Result<(), VerifyError> {
    let what = format!("{}@{}", package.name, package.version);
    let key = keys
        .iter()
        .find(|k| k.keyid == package.signature.keyid)
        .ok_or_else(|| VerifyError::Signature(what.clone()))?;
    let spki = STANDARD
        .decode(&key.key)
        .map_err(|_| VerifyError::Malformed(format!("registry key {}", key.keyid)))?;
    // A P-256 SubjectPublicKeyInfo ends with the uncompressed point.
    if spki.len() < 65 || spki[spki.len() - 65] != 0x04 {
        return Err(VerifyError::Malformed(format!(
            "registry key {}",
            key.keyid
        )));
    }
    let point = &spki[spki.len() - 65..];
    let sig = STANDARD
        .decode(&package.signature.sig)
        .map_err(|_| VerifyError::Signature(what.clone()))?;
    let message = format!("{}@{}:{}", package.name, package.version, package.integrity);
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point)
        .verify(message.as_bytes(), &sig)
        .map_err(|_| VerifyError::Signature(what))
}

fn stream(path: &Path, mut each: impl FnMut(&[u8])) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        each(&buf[..n]);
    }
}
