use std::collections::HashMap;
use std::sync::Mutex;

use super::{credential, delete_secrets, save_secrets, CredentialError, SecretStore};
use crate::connectors::definition::{Auth, Placement, StaticField};

/// Secrets in memory. `broken` makes every call fail like a locked keychain.
#[derive(Default)]
struct Memory {
    values: Mutex<HashMap<String, String>>,
    broken: bool,
}

impl Memory {
    fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.values.lock().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    fn fail(&self) -> Result<(), String> {
        if self.broken {
            return Err("keychain locked".into());
        }
        Ok(())
    }
}

impl SecretStore for Memory {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        self.fail()?;
        Ok(self.values.lock().unwrap().get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.fail()?;
        self.values
            .lock()
            .unwrap()
            .insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.fail()?;
        self.values.lock().unwrap().remove(name);
        Ok(())
    }
}

fn bearer() -> Auth {
    Auth::Static {
        fields: vec![StaticField {
            name: "api_key".into(),
            placement: Placement::Header,
            header: "Authorization".into(),
            format: "Bearer {}".into(),
        }],
    }
}

fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn a_saved_key_comes_back_as_its_header() {
    let store = Memory::default();
    save_secrets(
        &store,
        "c1",
        &bearer(),
        &values(&[("api_key", " lin_123 ")]),
    )
    .unwrap();
    assert_eq!(store.names(), ["connector:c1:api_key"]);

    let sent = credential(&store, "c1", &bearer()).unwrap().unwrap();
    let (name, value) = sent.parts();
    assert_eq!(name, "authorization");
    assert_eq!(value, "Bearer lin_123");
    assert!(value.is_sensitive());
    // The secret never shows in a debug print.
    assert!(!format!("{sent:?}").contains("lin_123"));
}

#[test]
fn a_connector_without_auth_needs_and_stores_nothing() {
    let store = Memory::default();
    save_secrets(&store, "c1", &Auth::None, &HashMap::new()).unwrap();
    assert!(credential(&store, "c1", &Auth::None).unwrap().is_none());
    assert!(store.names().is_empty());
}

#[test]
fn bad_values_store_nothing() {
    let store = Memory::default();
    let cases = [
        (values(&[]), "credential_missing"),
        (values(&[("api_key", "   ")]), "credential_missing"),
        (values(&[("api_key", "a\nb")]), "invalid_credential"),
        (
            values(&[("api_key", "k"), ("org", "x")]),
            "invalid_credential",
        ),
    ];
    for (input, code) in cases {
        let err = save_secrets(&store, "c1", &bearer(), &input).unwrap_err();
        assert_eq!(err.code(), code, "{input:?}");
        assert!(!err.to_string().contains("a\nb"));
    }
    assert!(store.names().is_empty());
}

/// A store that won't open is never read as "no secret", and saving fails
/// rather than going anywhere else.
#[test]
fn an_unavailable_store_is_an_error_of_its_own() {
    let store = Memory {
        broken: true,
        ..Default::default()
    };
    let err = credential(&store, "c1", &bearer()).unwrap_err();
    assert_eq!(err.code(), "credential_store_unavailable");
    let err = save_secrets(&store, "c1", &bearer(), &values(&[("api_key", "k")])).unwrap_err();
    assert_eq!(err.code(), "credential_store_unavailable");

    let empty = Memory::default();
    let err = credential(&empty, "c1", &bearer()).unwrap_err();
    assert_eq!(err, CredentialError::Missing("api_key".into()));
}

#[test]
fn deleting_secrets_can_run_again() {
    let store = Memory::default();
    save_secrets(&store, "c1", &bearer(), &values(&[("api_key", "k1")])).unwrap();
    save_secrets(&store, "c2", &bearer(), &values(&[("api_key", "k2")])).unwrap();
    delete_secrets(&store, "c1", &bearer()).unwrap();
    delete_secrets(&store, "c1", &bearer()).unwrap();
    assert_eq!(store.names(), ["connector:c2:api_key"]);
    assert!(credential(&store, "c2", &bearer()).unwrap().is_some());
}
