//! A connection's credentials (design r3 §5.5). Secret values live only in
//! the OS keychain (or a headless Host's opted-in file store), under
//! `connector:<connection id>:<field>`; the database and the definition hold
//! only which header carries them. A store that won't open is an error of
//! its own, never read as "no secret", and nothing falls back to another
//! store.

use std::collections::HashMap;

use http::HeaderName;

use super::definition::{Auth, StaticField};
use super::egress::Credential;

/// Where secret values are kept. [`Keychain`] in the Host; tests use their own.
pub trait SecretStore: Send + Sync {
    /// `Ok(None)` is "nothing stored"; `Err` is "the store would not open".
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    /// Deleting what isn't there is fine.
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// The Host's credential store: the OS keychain, or `tokens.json` under
/// `ZWIIM_HOST_KEY_STORE=file`.
pub struct Keychain;

impl SecretStore for Keychain {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        crate::keyring_store::get_secret(name)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        crate::keyring_store::set_secret(name, value)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        crate::keyring_store::delete_secret(name)
    }
}

/// Never carries a secret value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("no value for credential {0:?}")]
    Missing(String),
    #[error("{0:?} isn't a credential this connector takes")]
    Unknown(String),
    #[error("the value for credential {0:?} can't be sent in a header")]
    Invalid(String),
    #[error("the credential store is unavailable: {0}")]
    StoreUnavailable(String),
}

impl CredentialError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Missing(_) => "credential_missing",
            Self::Unknown(_) | Self::Invalid(_) => "invalid_credential",
            Self::StoreUnavailable(_) => "credential_store_unavailable",
        }
    }
}

fn secret_name(connection_id: &str, field: &str) -> String {
    format!("connector:{connection_id}:{field}")
}

fn fields(auth: &Auth) -> &[StaticField] {
    match auth {
        Auth::Static { fields } => fields,
        Auth::None | Auth::Oauth { .. } => &[],
    }
}

fn header(field: &StaticField, value: &str) -> Result<Credential, CredentialError> {
    let invalid = || CredentialError::Invalid(field.name.clone());
    let name = HeaderName::try_from(field.header.as_str()).map_err(|_| invalid())?;
    Credential::new(name, &field.format.replacen("{}", value, 1)).map_err(|_| invalid())
}

/// Stores the values the user entered for `auth`'s fields. Every value is
/// checked before anything is written; a field left out, an extra one or a
/// value that can't go in a header stores nothing.
pub fn save_secrets(
    store: &dyn SecretStore,
    connection_id: &str,
    auth: &Auth,
    values: &HashMap<String, String>,
) -> Result<(), CredentialError> {
    let fields = fields(auth);
    if let Some(extra) = values
        .keys()
        .find(|k| !fields.iter().any(|f| &f.name == *k))
    {
        return Err(CredentialError::Unknown(extra.clone()));
    }
    let mut checked = Vec::new();
    for field in fields {
        let value = values
            .get(&field.name)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CredentialError::Missing(field.name.clone()))?;
        header(field, value)?;
        checked.push((secret_name(connection_id, &field.name), value));
    }
    for (name, value) in checked {
        store
            .set(&name, value)
            .map_err(CredentialError::StoreUnavailable)?;
    }
    Ok(())
}

/// The credential to send to the connection's credentialed endpoints, or
/// `None` when it needs none. Egress sends it only to those endpoints.
pub fn credential(
    store: &dyn SecretStore,
    connection_id: &str,
    auth: &Auth,
) -> Result<Option<Credential>, CredentialError> {
    let Some(field) = fields(auth).first() else {
        return Ok(None);
    };
    let value = store
        .get(&secret_name(connection_id, &field.name))
        .map_err(CredentialError::StoreUnavailable)?
        .ok_or_else(|| CredentialError::Missing(field.name.clone()))?;
    header(field, &value).map(Some)
}

/// Deletes every stored value of `auth`'s fields; a value already gone is
/// fine, so revocation can run this step again.
pub fn delete_secrets(
    store: &dyn SecretStore,
    connection_id: &str,
    auth: &Auth,
) -> Result<(), CredentialError> {
    for field in fields(auth) {
        store
            .delete(&secret_name(connection_id, &field.name))
            .map_err(CredentialError::StoreUnavailable)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
