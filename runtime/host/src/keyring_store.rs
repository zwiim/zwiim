// Modified from upstream Apache-2.0 source; see NOTICE.
const SERVICE_NAME: &str = "zwiim";

fn token_key(account_id: &str) -> String {
    format!("github-token:{}", account_id)
}

fn channel_token_key(channel_id: i32) -> String {
    format!("chat-channel:{}", channel_id)
}

/// Namespace for secrets that are not tokens of an account or a channel. The
/// prefix keeps them from ever colliding with a `github-token:<id>` whose id
/// happens to look like a secret name.
fn secret_key(name: &str) -> String {
    format!("secret:{}", name)
}

// ── Tauri mode: OS keyring ──

#[cfg(feature = "tauri-runtime")]
pub fn set_token(account_id: &str, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_token(account_id: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

// ── Host: OS keychain, or the file store a headless Host opts into ──

#[cfg(not(feature = "tauri-runtime"))]
fn tokens_file_path() -> std::path::PathBuf {
    tokens_file_path_for(std::env::var("ZWIIM_DATA_DIR").ok().as_deref())
}

/// Resolve the on-disk `tokens.json` path given an explicit
/// `ZWIIM_DATA_DIR` value (or `None` to fall back to the platform
/// default). Always returns an absolute path so subprocess credential
/// helpers — which inherit our env but run in git's CWD, not ours —
/// don't end up looking for `tokens.json` in the user's repo. Factored
/// out so tests can exercise path resolution without poking at process
/// env state.
#[cfg(not(feature = "tauri-runtime"))]
fn tokens_file_path_for(env_value: Option<&str>) -> std::path::PathBuf {
    let dir = env_value.map(std::path::PathBuf::from).unwrap_or_else(|| {
        dirs::data_dir()
            .map(|d| d.join("zwiim"))
            .unwrap_or_else(|| std::path::PathBuf::from(".zwiim-data"))
    });
    crate::git_credential::absolutize(&dir).join("tokens.json")
}

#[cfg(not(feature = "tauri-runtime"))]
fn read_tokens() -> std::collections::HashMap<String, String> {
    read_tokens_at(&tokens_file_path())
}

/// Read the token map, first tightening a pre-existing file to `0600`.
/// Stores written before the permission hardening landed sit at the umask
/// default (usually 0644) inside a bind-mounted `/data` volume; tightening on
/// every read is idempotent and cheap, and doing it BEFORE the read means no
/// code path ever handles token bytes from a world-readable file it could have
/// fixed. Best-effort: if chmod fails the read will usually fail too, and a
/// read-only mount is not made worse by proceeding.
#[cfg(not(feature = "tauri-runtime"))]
fn read_tokens_at(path: &std::path::Path) -> std::collections::HashMap<String, String> {
    read_tokens_at_checked(path).unwrap_or_default()
}

/// The same read, but an unreadable or corrupt store is an error rather than an
/// empty map. Callers that WRITE secrets back need the distinction: "there is no
/// entry" and "the file would not open" lead to opposite decisions on the way
/// out — write nothing, versus refuse to write at all — and collapsing them
/// turns a transient read failure into a permanent deletion.
#[cfg(not(feature = "tauri-runtime"))]
fn read_tokens_at_checked(
    path: &std::path::Path,
) -> Result<std::collections::HashMap<String, String>, String> {
    #[cfg(unix)]
    if path.exists() {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) =
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        {
            // Keep reading (a read-only mount is not made worse), but make the
            // failed hardening observable instead of silently world-readable.
            tracing::warn!(
                "[tokens] could not tighten {} to 0600: {err}",
                path.display()
            );
        }
    }
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("token store is not readable JSON: {e}")),
        // A store that was never written is legitimately empty.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(std::collections::HashMap::new())
        }
        Err(err) => Err(format!("token store read error: {err}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
fn write_tokens(tokens: &std::collections::HashMap<String, String>) -> Result<(), String> {
    write_tokens_at(&tokens_file_path(), tokens)
}

/// Persist the token map without ever exposing a wide-permission file, even
/// transiently. A plain `fs::write` + chmod leaves a window (and a permanent
/// 0644 file if the process dies between the two), so on Unix the content goes
/// into a same-directory temp file created with mode `0600`, is fsynced, and
/// then atomically renamed over the store. The explicit `set_permissions`
/// after creation pins the bits exactly even under an exotic umask (umask can
/// only clear bits at open time; chmod is not masked).
#[cfg(not(feature = "tauri-runtime"))]
fn write_tokens_at(
    path: &std::path::Path,
    tokens: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "token store path has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create token store directory: {e}"))?;
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("failed to serialize tokens: {e}"))?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        use std::sync::atomic::{AtomicU64, Ordering};
        static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = parent.join(format!(
            ".tokens.json.tmp-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("failed to create token store temp file: {e}"))?;
        let write_result = file
            .write_all(json.as_bytes())
            .and_then(|()| file.set_permissions(std::fs::Permissions::from_mode(0o600)))
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("failed to write token store: {e}"))
            .and_then(|()| {
                std::fs::rename(&tmp, path)
                    .map_err(|e| format!("failed to persist token store: {e}"))
            });
        if write_result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        write_result
    }

    #[cfg(not(unix))]
    {
        // Non-Unix server builds are outside the supported surface; keep the
        // plain write rather than pretending NTFS ACL hardening exists here.
        std::fs::write(path, json).map_err(|e| format!("failed to write token store: {e}"))
    }
}

/// Whether credentials live in the OS keychain. This is the choice the Host's
/// own keys make: the keychain, unless `ZWIIM_HOST_KEY_STORE=file` opts a
/// headless Host into `tokens.json`. Unit tests never touch the real keychain.
#[cfg(not(feature = "tauri-runtime"))]
fn uses_keychain() -> bool {
    let selected = std::env::var(crate::identity::host_keys::KEY_STORE_ENV).ok();
    !cfg!(test) && selected.as_deref().map(str::trim) != Some("file")
}

#[cfg(not(feature = "tauri-runtime"))]
fn keychain_entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE_NAME, key).map_err(|e| format!("keyring init error: {e}"))
}

#[cfg(not(feature = "tauri-runtime"))]
fn keychain_get(key: &str) -> Result<Option<String>, String> {
    match keychain_entry(key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring get error: {e}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
fn keychain_set(key: &str, value: &str) -> Result<(), String> {
    keychain_entry(key)?
        .set_password(value)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(not(feature = "tauri-runtime"))]
fn keychain_delete(key: &str) -> Result<(), String> {
    match keychain_entry(key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

/// Drop `key` from `tokens.json`, without creating the file when it has none.
#[cfg(not(feature = "tauri-runtime"))]
fn remove_from_file(key: &str) -> Result<(), String> {
    let path = tokens_file_path();
    let mut tokens = read_tokens_at_checked(&path)?;
    if tokens.remove(key).is_some() {
        write_tokens_at(&path, &tokens)?;
    }
    Ok(())
}

/// The keychain first, then `tokens.json`: a credential stays readable until
/// [`migrate_file_store`] has moved it, even if the keychain refuses it. A
/// keychain failure is only reported when the file has no answer either.
#[cfg(not(feature = "tauri-runtime"))]
fn store_get(key: &str) -> Result<Option<String>, String> {
    let keychain = if uses_keychain() { keychain_get(key) } else { Ok(None) };
    if matches!(keychain, Ok(Some(_))) {
        return keychain;
    }
    let from_file = read_tokens_at_checked(&tokens_file_path())?.remove(key);
    from_file.map_or(keychain, |value| Ok(Some(value)))
}

#[cfg(not(feature = "tauri-runtime"))]
fn store_set(key: &str, value: &str) -> Result<(), String> {
    if uses_keychain() {
        keychain_set(key, value)?;
        return remove_from_file(key);
    }
    let mut tokens = read_tokens();
    tokens.insert(key.to_string(), value.to_string());
    write_tokens(&tokens)
}

#[cfg(not(feature = "tauri-runtime"))]
fn store_delete(key: &str) -> Result<(), String> {
    if uses_keychain() {
        keychain_delete(key)?;
    }
    remove_from_file(key)
}

/// Offer every entry to `put`; what it refuses stays behind.
#[cfg(not(feature = "tauri-runtime"))]
fn move_entries(
    tokens: std::collections::HashMap<String, String>,
    mut put: impl FnMut(&str, &str) -> Result<(), String>,
) -> std::collections::HashMap<String, String> {
    tokens
        .into_iter()
        .filter(|(key, value)| match put(key, value) {
            Ok(()) => false,
            Err(err) => {
                tracing::warn!("[tokens] kept {key} in the file store: {err}");
                true
            }
        })
        .collect()
}

/// `tokens.json` after a move: gone once empty, so no credential file lingers.
#[cfg(not(feature = "tauri-runtime"))]
fn write_remaining(
    path: &std::path::Path,
    remaining: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    if !remaining.is_empty() {
        return write_tokens_at(path, remaining);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("token store remove error: {err}")),
    }
}

/// Move what an older Host left in `tokens.json` into the keychain. An entry
/// leaves the file only once the keychain reads it back, so a denied prompt
/// loses nothing and the next start tries again.
#[cfg(not(feature = "tauri-runtime"))]
pub fn migrate_file_store() {
    if !uses_keychain() {
        return;
    }
    let path = tokens_file_path();
    let tokens = match read_tokens_at_checked(&path) {
        Ok(tokens) if tokens.is_empty() => return,
        Ok(tokens) => tokens,
        Err(err) => return tracing::warn!("[tokens] file store not migrated: {err}"),
    };
    let remaining = move_entries(tokens, |key, value| {
        keychain_set(key, value)?;
        match keychain_get(key)? {
            Some(stored) if stored == value => Ok(()),
            _ => Err("the keychain did not return the saved value".to_string()),
        }
    });
    if let Err(err) = write_remaining(&path, &remaining) {
        tracing::warn!("[tokens] file store not updated: {err}");
    }
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_token(account_id: &str, token: &str) -> Result<(), String> {
    store_set(&token_key(account_id), token)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_token(account_id: &str) -> Option<String> {
    store_get(&token_key(account_id)).ok().flatten()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    store_delete(&token_key(account_id))
}

// ── Chat channel token helpers ──
// Reuse the same storage mechanism (keyring or file) with a different key prefix.

#[cfg(feature = "tauri-runtime")]
pub fn set_channel_token(channel_id: i32, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_channel_token(channel_id: i32) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_channel_token(channel_id: i32) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_channel_token(channel_id: i32, token: &str) -> Result<(), String> {
    store_set(&channel_token_key(channel_id), token)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_channel_token(channel_id: i32) -> Option<String> {
    store_get(&channel_token_key(channel_id)).ok().flatten()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_channel_token(channel_id: i32) -> Result<(), String> {
    store_delete(&channel_token_key(channel_id))
}

// ── Named secrets ──
// Same storage as the tokens above (the OS keychain, or the 0600 `tokens.json`
// a headless Host opts into), for credentials that belong to a feature rather
// than to an account: the config-sync WebDAV password and snapshot passphrase.

#[cfg(feature = "tauri-runtime")]
pub fn set_secret(name: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &secret_key(name))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(value)
        .map_err(|e| format!("keyring set error: {e}"))
}

/// `Ok(None)` is "nothing stored under that name"; `Err` is "the store would not
/// open". Unlike [`get_token`], which collapses both into `None`, a secret's
/// reader has to keep them apart: an empty value means "delete this entry" when
/// it travels back through [`set_secret`]/[`delete_secret`], so a denied
/// keychain prompt read as "absent" would erase the secret at the next save.
#[cfg(feature = "tauri-runtime")]
pub fn get_secret(name: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &secret_key(name))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring get error: {e}")),
    }
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_secret(name: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &secret_key(name))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_secret(name: &str, value: &str) -> Result<(), String> {
    store_set(&secret_key(name), value)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_secret(name: &str) -> Result<Option<String>, String> {
    store_get(&secret_key(name))
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_secret(name: &str) -> Result<(), String> {
    store_delete(&secret_key(name))
}

#[cfg(all(test, not(feature = "tauri-runtime")))]
mod tests {
    use super::*;

    #[test]
    fn test_tokens_file_path_absolutizes_relative_env() {
        // Regression: a relative `ZWIIM_DATA_DIR=data` previously made
        // `tokens.json` resolve against the helper subprocess's CWD (i.e.
        // git's repo dir), even after we'd absolutized the path used for
        // the database. The token store must always land on an absolute
        // path so DB lookup and token lookup point at the same root.
        let cwd = std::env::current_dir().expect("cwd");
        let resolved = tokens_file_path_for(Some("data"));
        assert!(
            resolved.is_absolute(),
            "tokens path must be absolute, got: {}",
            resolved.display()
        );
        assert_eq!(resolved, cwd.join("data").join("tokens.json"));
    }

    #[test]
    fn test_tokens_file_path_absolute_env_unchanged() {
        let data_dir = std::env::current_dir().expect("cwd").join("zwiim-data");
        let data_dir_str = data_dir.to_string_lossy().to_string();
        let resolved = tokens_file_path_for(Some(&data_dir_str));
        assert_eq!(resolved, data_dir.join("tokens.json"));
    }

    #[test]
    fn test_tokens_file_path_default_when_unset() {
        // No env var → derived from `dirs::data_dir()` (always absolute on
        // every platform we ship to). Just verify we end at `tokens.json`
        // and that the result is absolute, not the literal default.
        let resolved = tokens_file_path_for(None);
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("tokens.json"));
    }

    #[cfg(unix)]
    fn mode_bits(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    /// A fresh store must be 0600 from its very first byte on disk — there is
    /// no window where a parallel reader could see a wide-permission file,
    /// because the temp file is created with the final mode and only then
    /// renamed into place.
    #[test]
    #[cfg(unix)]
    fn test_write_tokens_creates_0600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        let mut tokens = std::collections::HashMap::new();
        tokens.insert("github-token:a".to_string(), "secret".to_string());
        write_tokens_at(&path, &tokens).expect("write");
        assert_eq!(mode_bits(&path), 0o600);
        assert_eq!(read_tokens_at(&path).get("github-token:a").unwrap(), "secret");
        // No temp residue left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tokens.json.tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must not survive a write");
    }

    /// Overwriting a legacy wide-permission store must end 0600: the rename
    /// replaces the inode, so the old 0644 bits die with the old file.
    #[test]
    #[cfg(unix)]
    fn test_write_tokens_replaces_legacy_wide_file_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        std::fs::write(&path, "{}").expect("seed legacy file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut tokens = std::collections::HashMap::new();
        tokens.insert("github-token:b".to_string(), "s2".to_string());
        write_tokens_at(&path, &tokens).expect("write");
        assert_eq!(mode_bits(&path), 0o600);
    }

    /// Reading an existing legacy store tightens it to 0600 before the bytes
    /// are consumed, so a server that only ever reads (never re-saves) still
    /// heals the volume-mounted file.
    #[test]
    #[cfg(unix)]
    fn test_read_tokens_tightens_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        std::fs::write(&path, r#"{"github-token:c":"s3"}"#).expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let tokens = read_tokens_at(&path);
        assert_eq!(tokens.get("github-token:c").unwrap(), "s3");
        assert_eq!(mode_bits(&path), 0o600);
    }

    #[test]
    fn a_refused_entry_stays_in_the_file_store() {
        let tokens = std::collections::HashMap::from([
            ("moved".to_string(), "a".to_string()),
            ("refused".to_string(), "b".to_string()),
        ]);
        let mut stored = Vec::new();
        let remaining = move_entries(tokens, |key, value| {
            if key == "refused" {
                return Err("denied".to_string());
            }
            stored.push((key.to_string(), value.to_string()));
            Ok(())
        });
        assert_eq!(stored, [("moved".to_string(), "a".to_string())]);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining.get("refused").unwrap(), "b");
    }

    #[test]
    fn an_empty_file_store_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let mut left = std::collections::HashMap::new();
        left.insert("refused".to_string(), "b".to_string());

        write_remaining(&path, &left).unwrap();
        assert_eq!(read_tokens_at(&path), left);

        left.clear();
        write_remaining(&path, &left).unwrap();
        assert!(!path.exists());
        write_remaining(&path, &left).unwrap();
    }

    /// Writes to the real keychain, so it runs only on request:
    /// `cargo test -p zwiim-host --lib keyring_store -- --ignored`.
    #[test]
    #[ignore = "uses the real OS keychain"]
    fn the_keychain_keeps_a_credential_until_it_is_deleted() {
        let key = format!("test-only:{}", uuid::Uuid::new_v4());
        assert_eq!(keychain_get(&key).unwrap(), None);
        keychain_set(&key, "value").unwrap();
        assert_eq!(keychain_get(&key).unwrap().as_deref(), Some("value"));
        keychain_delete(&key).unwrap();
        assert_eq!(keychain_get(&key).unwrap(), None);
        keychain_delete(&key).unwrap();
    }
}
