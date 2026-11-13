//! The Cloud's deployment configuration, read from the environment and
//! checked before anything starts. Provider-independent: the same settings
//! run the Cloud on a laptop behind a tunnel or on a server with its own TLS.
//!
//! | variable | |
//! |---|---|
//! | `ZWIIM_CLOUD_PUBLIC_URL` | required: the origin hosts and devices use (`https://…`; `http` on loopback only) |
//! | `ZWIIM_CLOUD_DATA_DIR` | required: registry directory (created `0700`) |
//! | `ZWIIM_CLOUD_ADDR` | listen address (default `127.0.0.1:8443`) |
//! | `ZWIIM_CLOUD_TLS_CERT` / `ZWIIM_CLOUD_TLS_KEY` | PEM files; required unless the address is loopback |
//! | `ZWIIM_CLOUD_SUPABASE_URL` | Supabase project URL: tokens verified against its JWKS and issuer |
//! | `ZWIIM_CLOUD_JWT_SECRET` | secret: the project's legacy HS256 JWT secret |
//! | `ZWIIM_CLOUD_JWT_AUDIENCE` | default `authenticated` |
//! | `ZWIIM_CLOUD_USAGE_DATABASE_URL` | secret: Postgres URL for the `zwiim_cloud` role; turns on relay usage storage and plan limits |
//! | `ZWIIM_CLOUD_USAGE_DATABASE_CA` | PEM file the usage database's certificate is checked against (default: system roots) |
//! | `ZWIIM_CLOUD_MAX_CONNECTIONS` | open connections, all kinds (default 1024) |
//! | `ZWIIM_CLOUD_MAX_RELAYS` | concurrent device relays (default 256) |
//! | `ZWIIM_CLOUD_MAX_CONNECTS_PER_IP` | new connections per client address per minute (default 120) |
//! | `ZWIIM_CLOUD_MAX_REQUESTS_PER_ACCOUNT` | authenticated requests per account per minute (default 300) |
//! | `ZWIIM_CLOUD_LOG` | log filter (else `RUST_LOG`, else `info`) |
//! | `ZWIIM_CLOUD_LOG_FORMAT` | `text` (default) or `json` |
//!
//! At least one of the Supabase URL and the JWT secret is required.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::AuthConfig;
use crate::proto::normalize_cloud_url;
use crate::server::{Config, Limits, Timing};
use crate::usage::PgStore;

const DEFAULT_ADDR: &str = "127.0.0.1:8443";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

/// Everything the binary needs, validated.
#[derive(Debug)]
pub struct Settings {
    pub addr: SocketAddr,
    pub public_url: String,
    pub data_dir: PathBuf,
    /// Certificate and key files, when the Cloud terminates TLS itself.
    pub tls: Option<(PathBuf, PathBuf)>,
    pub auth: AuthConfig,
    pub limits: Limits,
    /// The usage database; its `Debug` hides the password.
    pub usage_database: Option<tokio_postgres::Config>,
    pub usage_database_ca: Option<PathBuf>,
    pub log_filter: String,
    pub log_format: LogFormat,
}

fn limit(get: &impl Fn(&str) -> Option<String>, name: &str, default: usize) -> Result<usize, String> {
    match get(name) {
        None => Ok(default),
        Some(raw) => raw
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=1_000_000).contains(n))
            .ok_or_else(|| format!("{name} must be a number from 1 to 1000000")),
    }
}

impl Settings {
    /// Read and check the settings. `get` returns a variable's value, with
    /// empty values treated as unset.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let get = |name: &str| get(name).filter(|v| !v.trim().is_empty());

        let addr_raw = get("ZWIIM_CLOUD_ADDR").unwrap_or_else(|| DEFAULT_ADDR.into());
        let addr: SocketAddr = addr_raw
            .trim()
            .parse()
            .map_err(|_| format!("ZWIIM_CLOUD_ADDR is not an address and port: {addr_raw}"))?;

        let public_url = get("ZWIIM_CLOUD_PUBLIC_URL").ok_or("ZWIIM_CLOUD_PUBLIC_URL is required")?;
        let public_url =
            normalize_cloud_url(&public_url).map_err(|e| format!("ZWIIM_CLOUD_PUBLIC_URL: {e}"))?;

        let data_dir = get("ZWIIM_CLOUD_DATA_DIR").map(PathBuf::from).ok_or("ZWIIM_CLOUD_DATA_DIR is required")?;

        let tls = match (get("ZWIIM_CLOUD_TLS_CERT"), get("ZWIIM_CLOUD_TLS_KEY")) {
            (Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
            (None, None) => None,
            _ => return Err("set both ZWIIM_CLOUD_TLS_CERT and ZWIIM_CLOUD_TLS_KEY, or neither".into()),
        };
        if tls.is_none() && !addr.ip().is_loopback() {
            return Err(format!(
                "ZWIIM_CLOUD_ADDR {addr} is not loopback: configure TLS, or listen on loopback behind a TLS-terminating proxy or tunnel"
            ));
        }

        let audience = get("ZWIIM_CLOUD_JWT_AUDIENCE").unwrap_or_else(|| "authenticated".into());
        let secret = get("ZWIIM_CLOUD_JWT_SECRET").map(String::into_bytes);
        let auth = match get("ZWIIM_CLOUD_SUPABASE_URL") {
            Some(url) => {
                let url = normalize_cloud_url(&url).map_err(|e| format!("ZWIIM_CLOUD_SUPABASE_URL: {e}"))?;
                AuthConfig::supabase(&url, secret, audience)
            }
            None => AuthConfig { hs256_secret: secret, audience, ..Default::default() },
        };
        if auth.hs256_secret.is_none() && auth.jwks_url.is_none() {
            return Err("set ZWIIM_CLOUD_SUPABASE_URL (JWKS) and/or ZWIIM_CLOUD_JWT_SECRET (HS256)".into());
        }

        let defaults = Limits::default();
        let limits = Limits {
            max_connections: limit(&get, "ZWIIM_CLOUD_MAX_CONNECTIONS", defaults.max_connections)?,
            max_relays: limit(&get, "ZWIIM_CLOUD_MAX_RELAYS", defaults.max_relays)?,
            max_connects_per_ip: limit(&get, "ZWIIM_CLOUD_MAX_CONNECTS_PER_IP", defaults.max_connects_per_ip)?,
            max_requests_per_account: limit(
                &get,
                "ZWIIM_CLOUD_MAX_REQUESTS_PER_ACCOUNT",
                defaults.max_requests_per_account,
            )?,
        };

        // The value is a secret: never echoed back.
        let usage_database = get("ZWIIM_CLOUD_USAGE_DATABASE_URL")
            .map(|url| url.trim().parse::<tokio_postgres::Config>())
            .transpose()
            .map_err(|_| "ZWIIM_CLOUD_USAGE_DATABASE_URL is not a Postgres connection URL".to_string())?;
        let usage_database_ca = get("ZWIIM_CLOUD_USAGE_DATABASE_CA").map(PathBuf::from);
        if usage_database_ca.is_some() && usage_database.is_none() {
            return Err("ZWIIM_CLOUD_USAGE_DATABASE_CA needs ZWIIM_CLOUD_USAGE_DATABASE_URL".into());
        }

        let log_filter = get("ZWIIM_CLOUD_LOG")
            .or_else(|| get("RUST_LOG"))
            .unwrap_or_else(|| "info".into());
        let log_format = match get("ZWIIM_CLOUD_LOG_FORMAT").as_deref().map(str::trim) {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(other) => return Err(format!("ZWIIM_CLOUD_LOG_FORMAT must be text or json, not {other}")),
        };

        Ok(Self { addr, public_url, data_dir, tls, auth, limits, usage_database, usage_database_ca, log_filter, log_format })
    }

    /// The service configuration.
    pub fn config(&self) -> Result<Config, String> {
        let usage_store = match &self.usage_database {
            Some(db) => Some(Arc::new(
                PgStore::new(db.clone(), self.usage_database_ca.as_deref())
                    .map_err(|e| format!("ZWIIM_CLOUD_USAGE_DATABASE_CA: {e}"))?,
            ) as Arc<dyn crate::usage::Store>),
            None => None,
        };
        Ok(Config {
            public_url: self.public_url.clone(),
            data_dir: self.data_dir.clone(),
            auth: self.auth.clone(),
            timing: Timing::default(),
            limits: self.limits,
            relay_tap: None,
            usage_store,
        })
    }

    /// One line for the startup log: public facts only, never the secret.
    pub fn summary(&self) -> String {
        let tls = if self.tls.is_some() { "terminated here" } else { "terminated upstream (loopback listener)" };
        let usage = if self.usage_database.is_some() { "database" } else { "memory only" };
        let auth = match (&self.auth.jwks_url, &self.auth.hs256_secret) {
            (Some(_), Some(_)) => "Supabase JWKS + HS256 secret",
            (Some(_), None) => "Supabase JWKS",
            (None, _) => "HS256 secret",
        };
        format!(
            "public_url={} addr={} tls={tls} auth={auth} issuer={} audience={} data_dir={} max_connections={} max_relays={} usage={usage}",
            self.public_url,
            self.addr,
            self.auth.issuer.as_deref().unwrap_or("-"),
            self.auth.audience,
            self.data_dir.display(),
            self.limits.max_connections,
            self.limits.max_relays,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(vars: &[(&str, &str)]) -> Result<Settings, String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Settings::from_env(|k| vars.get(k).cloned())
    }

    const BASE: [(&str, &str); 3] = [
        ("ZWIIM_CLOUD_PUBLIC_URL", "https://cloud.example.com/"),
        ("ZWIIM_CLOUD_DATA_DIR", "/var/lib/zwiim-cloud"),
        ("ZWIIM_CLOUD_SUPABASE_URL", "https://abc.supabase.co"),
    ];

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut vars: Vec<_> = BASE.iter().copied().filter(|(k, _)| !extra.iter().any(|(e, _)| e == k)).collect();
        vars.extend_from_slice(extra);
        vars
    }

    #[test]
    fn a_tunnelled_development_cloud() {
        let s = settings(&BASE).unwrap();
        assert_eq!(s.addr, "127.0.0.1:8443".parse().unwrap());
        assert_eq!(s.public_url, "https://cloud.example.com");
        assert!(s.tls.is_none());
        assert_eq!(s.auth.jwks_url.as_deref(), Some("https://abc.supabase.co/auth/v1/.well-known/jwks.json"));
        assert_eq!(s.auth.issuer.as_deref(), Some("https://abc.supabase.co/auth/v1"));
        assert_eq!(s.auth.audience, "authenticated");
        assert_eq!((s.limits.max_connections, s.limits.max_relays), (1024, 256));
        assert_eq!((s.log_filter.as_str(), s.log_format), ("info", LogFormat::Text));
        assert_eq!(s.config().unwrap().public_url, "https://cloud.example.com");
        assert!(s.usage_database.is_none());
        assert!(s.summary().contains("usage=memory only"));
    }

    #[test]
    fn a_server_with_its_own_tls_and_limits() {
        let s = settings(&with(&[
            ("ZWIIM_CLOUD_ADDR", "0.0.0.0:443"),
            ("ZWIIM_CLOUD_TLS_CERT", "/etc/zwiim/cert.pem"),
            ("ZWIIM_CLOUD_TLS_KEY", "/etc/zwiim/key.pem"),
            ("ZWIIM_CLOUD_MAX_CONNECTIONS", "4096"),
            ("ZWIIM_CLOUD_MAX_RELAYS", "1000"),
            ("ZWIIM_CLOUD_LOG", "zwiim_cloud=debug"),
            ("ZWIIM_CLOUD_LOG_FORMAT", "json"),
        ]))
        .unwrap();
        assert!(s.tls.is_some());
        assert_eq!((s.limits.max_connections, s.limits.max_relays), (4096, 1000));
        assert_eq!((s.log_filter.as_str(), s.log_format), ("zwiim_cloud=debug", LogFormat::Json));
    }

    #[test]
    fn bad_settings_are_refused_with_the_variable_named() {
        let cases: [(&[(&str, &str)], &str); 13] = [
            (&[("ZWIIM_CLOUD_PUBLIC_URL", "")], "ZWIIM_CLOUD_PUBLIC_URL is required"),
            (&[("ZWIIM_CLOUD_PUBLIC_URL", "http://cloud.example.com")], "ZWIIM_CLOUD_PUBLIC_URL"),
            (&[("ZWIIM_CLOUD_PUBLIC_URL", "https://cloud.example.com/zwiim")], "ZWIIM_CLOUD_PUBLIC_URL"),
            (&[("ZWIIM_CLOUD_DATA_DIR", " ")], "ZWIIM_CLOUD_DATA_DIR"),
            (&[("ZWIIM_CLOUD_ADDR", "8443")], "ZWIIM_CLOUD_ADDR"),
            (&[("ZWIIM_CLOUD_ADDR", "0.0.0.0:8443")], "not loopback"),
            (&[("ZWIIM_CLOUD_TLS_CERT", "/c.pem")], "ZWIIM_CLOUD_TLS_KEY"),
            (&[("ZWIIM_CLOUD_SUPABASE_URL", "")], "ZWIIM_CLOUD_JWT_SECRET"),
            (&[("ZWIIM_CLOUD_SUPABASE_URL", "http://abc.supabase.co")], "ZWIIM_CLOUD_SUPABASE_URL"),
            (&[("ZWIIM_CLOUD_MAX_RELAYS", "0")], "ZWIIM_CLOUD_MAX_RELAYS"),
            (&[("ZWIIM_CLOUD_LOG_FORMAT", "yaml")], "ZWIIM_CLOUD_LOG_FORMAT"),
            (&[("ZWIIM_CLOUD_USAGE_DATABASE_URL", "postgres://u:p@db:5432/x?sslmode=bogus")], "ZWIIM_CLOUD_USAGE_DATABASE_URL"),
            (&[("ZWIIM_CLOUD_USAGE_DATABASE_CA", "/ca.pem")], "needs ZWIIM_CLOUD_USAGE_DATABASE_URL"),
        ];
        for (vars, expected) in cases {
            let err = settings(&with(vars)).unwrap_err();
            assert!(err.contains(expected), "{vars:?}: {err}");
        }
    }

    #[test]
    fn a_local_supabase_and_a_secret_only_cloud_are_fine() {
        let s = settings(&with(&[("ZWIIM_CLOUD_SUPABASE_URL", "http://127.0.0.1:54321")])).unwrap();
        assert_eq!(s.auth.issuer.as_deref(), Some("http://127.0.0.1:54321/auth/v1"));
        let s = settings(&with(&[("ZWIIM_CLOUD_SUPABASE_URL", ""), ("ZWIIM_CLOUD_JWT_SECRET", "s3cret-value")])).unwrap();
        assert!(s.auth.jwks_url.is_none() && s.auth.hs256_secret.is_some());
    }

    #[test]
    fn the_secret_never_appears_in_the_summary_or_debug_output() {
        let s = settings(&with(&[("ZWIIM_CLOUD_JWT_SECRET", "super-secret-jwt-value")])).unwrap();
        assert!(s.summary().contains("auth=Supabase JWKS + HS256 secret"), "{}", s.summary());
        assert!(!s.summary().contains("super-secret-jwt-value"));
        assert!(!format!("{s:?}").contains("super-secret-jwt-value"));
        assert!(!format!("{s:?}").contains(&format!("{:?}", b"super-secret-jwt-value")));
    }

    #[test]
    fn the_usage_database_password_never_appears_either() {
        let url = "postgres://zwiim_cloud.ref:db-password-value@pooler.example.com:5432/postgres";
        let s = settings(&with(&[("ZWIIM_CLOUD_USAGE_DATABASE_URL", url)])).unwrap();
        assert!(s.summary().contains("usage=database"), "{}", s.summary());
        assert!(!s.summary().contains("db-password-value"));
        assert!(!format!("{s:?}").contains("db-password-value"));
        let err = settings(&with(&[("ZWIIM_CLOUD_USAGE_DATABASE_URL", "postgres://u:db-password-value@h/x?sslmode=bogus")]))
            .unwrap_err();
        assert!(!err.contains("db-password-value"), "{err}");
    }
}
