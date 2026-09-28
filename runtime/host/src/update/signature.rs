//! Minisign verification of Zwiim release metadata.
//!
//! Upstream-derived: adapted from upstream `src-tauri/src/update/verify.rs`
//! (v0.31.1; see third_party/codeg/PROVENANCE.md). Kept: the wire format and
//! the `minisign-verify` call. Changed: the upstream embedded key is gone;
//! the Zwiim release key is compiled in from `ZWIIM_RELEASE_PUBKEY` by the
//! release build, and a build without it verifies nothing (updates off).
//!
//! One key signs everything a release publishes: `release.json`, and the
//! Desktop updater archives (`tauri signer sign`). The public key and every
//! signature use the Tauri form, base64 of a minisign text file, so the same
//! string is the Tauri updater's `pubkey`.

use base64::Engine;
use minisign_verify::{PublicKey, Signature};

/// The release public key this build trusts, if it was built with one.
const RELEASE_PUBKEY: Option<&str> = option_env!("ZWIIM_RELEASE_PUBKEY");

/// Decode the outer base64 wrapper into the inner minisign text.
fn unwrap_base64(b64: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("invalid utf-8 in minisign text: {e}"))
}

/// A public key from its Tauri form (base64 of the two-line `.pub` file).
pub fn parse_public_key(tauri_pubkey_b64: &str) -> Result<PublicKey, String> {
    let text = unwrap_base64(tauri_pubkey_b64)?;
    let key_line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .ok_or_else(|| "minisign public key text has no key line".to_string())?;
    PublicKey::from_base64(key_line)
        .map_err(|e| format!("failed to parse minisign public key: {e}"))
}

/// The release key compiled into this build. `None`: this build was not made
/// by the release pipeline and trusts no release.
pub fn release_key() -> Option<Result<PublicKey, String>> {
    RELEASE_PUBKEY
        .filter(|k| !k.trim().is_empty())
        .map(parse_public_key)
}

/// Verify `data` against a Tauri-form signature (base64 of a `.minisig`).
pub fn verify(key: &PublicKey, data: &[u8], tauri_sig_b64: &str) -> Result<(), String> {
    let text = unwrap_base64(tauri_sig_b64)?;
    let signature =
        Signature::decode(&text).map_err(|e| format!("failed to parse signature: {e}"))?;
    // `allow_legacy` as tauri-plugin-updater does: a valid signature from
    // this key is still required, prehashed or not.
    key.verify(data, &signature, true)
        .map_err(|e| format!("signature verification failed: {e}"))
}
