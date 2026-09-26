//! The account authentication boundary: Supabase access tokens.
//!
//! A token is accepted if it is signed by the project's JWT secret (HS256) or
//! by a key from the project's JWKS (ES256 / RS256), is unexpired, carries
//! the configured audience (and issuer, when a Supabase URL is configured),
//! and names a user in `sub`. The user id is all the Cloud takes from it:
//! tokens are never stored or logged, and grant nothing on a host.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, JwkSet, KeyAlgorithm};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

/// Unknown key ids trigger a JWKS refresh at most this often.
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(30);
const JWKS_TIMEOUT: Duration = Duration::from_secs(10);
const LEEWAY_SECS: u64 = 30;

#[derive(Clone, Default)]
pub struct AuthConfig {
    /// The project's JWT secret, for HS256 tokens.
    pub hs256_secret: Option<Vec<u8>>,
    /// The project's JWKS, for ES256 / RS256 tokens.
    pub jwks_url: Option<String>,
    /// Required `iss`, if set.
    pub issuer: Option<String>,
    /// Required `aud` (Supabase: `authenticated`).
    pub audience: String,
}

/// Never prints the secret.
impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthConfig")
            .field("hs256_secret", &self.hs256_secret.as_ref().map(|_| "<redacted>"))
            .field("jwks_url", &self.jwks_url)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish()
    }
}

impl AuthConfig {
    /// A Supabase project at `url` (`https://<ref>.supabase.co`).
    pub fn supabase(url: &str, hs256_secret: Option<Vec<u8>>, audience: String) -> Self {
        let base = url.trim_end_matches('/');
        Self {
            hs256_secret,
            jwks_url: Some(format!("{base}/auth/v1/.well-known/jwks.json")),
            issuer: Some(format!("{base}/auth/v1")),
            audience,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("missing bearer token")]
    Missing,
    #[error("invalid or expired token")]
    Invalid,
}

pub struct Verifier {
    hs256: Option<DecodingKey>,
    jwks: Option<Jwks>,
    issuer: Option<String>,
    audience: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
}

impl Verifier {
    pub fn new(config: AuthConfig) -> Result<Self, String> {
        if config.hs256_secret.is_none() && config.jwks_url.is_none() {
            return Err("no token verification configured (a JWT secret or a Supabase URL)".into());
        }
        if config.audience.is_empty() {
            return Err("the token audience must not be empty".into());
        }
        Ok(Self {
            hs256: config.hs256_secret.as_deref().map(DecodingKey::from_secret),
            jwks: config.jwks_url.map(Jwks::new),
            issuer: config.issuer,
            audience: config.audience,
        })
    }

    /// The user id of a valid token.
    pub async fn verify(&self, token: &str) -> Result<String, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Invalid)?;
        let key = match header.alg {
            Algorithm::HS256 => self.hs256.clone().ok_or(AuthError::Invalid)?,
            Algorithm::ES256 | Algorithm::RS256 => {
                let jwks = self.jwks.as_ref().ok_or(AuthError::Invalid)?;
                let kid = header.kid.as_deref().ok_or(AuthError::Invalid)?;
                match jwks.key(kid).await {
                    Some((key, alg)) if alg == header.alg => key,
                    _ => return Err(AuthError::Invalid),
                }
            }
            _ => return Err(AuthError::Invalid),
        };
        let mut validation = Validation::new(header.alg);
        validation.leeway = LEEWAY_SECS;
        validation.set_audience(&[&self.audience]);
        validation.set_required_spec_claims(&["exp", "sub", "aud"]);
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer]);
        }
        let sub = decode::<Claims>(token, &key, &validation)
            .map_err(|_| AuthError::Invalid)?
            .claims
            .sub;
        if sub.is_empty() || sub.len() > 128 || sub.chars().any(char::is_control) {
            return Err(AuthError::Invalid);
        }
        Ok(sub)
    }

    /// The user id behind an `Authorization: Bearer` header value.
    pub async fn verify_header(&self, header: Option<&str>) -> Result<String, AuthError> {
        let token = header
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(AuthError::Missing)?;
        self.verify(token).await
    }
}

struct Jwks {
    url: String,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, (DecodingKey, Algorithm)>>,
    last_fetch: tokio::sync::Mutex<Option<Instant>>,
}

impl Jwks {
    fn new(url: String) -> Self {
        Self {
            url,
            http: reqwest::Client::builder()
                .timeout(JWKS_TIMEOUT)
                .build()
                .expect("an HTTP client"),
            keys: RwLock::default(),
            last_fetch: tokio::sync::Mutex::new(None),
        }
    }

    fn cached(&self, kid: &str) -> Option<(DecodingKey, Algorithm)> {
        self.keys.read().unwrap().get(kid).cloned()
    }

    async fn key(&self, kid: &str) -> Option<(DecodingKey, Algorithm)> {
        if let Some(key) = self.cached(kid) {
            return Some(key);
        }
        // An unknown key id: the project may have rotated keys. Refresh,
        // but not on every bogus token.
        let mut last = self.last_fetch.lock().await;
        if let Some(key) = self.cached(kid) {
            return Some(key);
        }
        if last.is_some_and(|t| t.elapsed() < JWKS_MIN_REFRESH) {
            return None;
        }
        *last = Some(Instant::now());
        match self.fetch().await {
            Ok(keys) => *self.keys.write().unwrap() = keys,
            Err(e) => tracing::warn!("[auth] JWKS fetch failed: {e}"),
        }
        self.cached(kid)
    }

    async fn fetch(&self) -> Result<HashMap<String, (DecodingKey, Algorithm)>, String> {
        let set: JwkSet = self
            .http
            .get(&self.url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.without_url().to_string())?
            .json()
            .await
            .map_err(|e| e.without_url().to_string())?;
        let mut keys = HashMap::new();
        for jwk in &set.keys {
            let Some(kid) = jwk.common.key_id.clone() else { continue };
            let alg = match (&jwk.common.key_algorithm, &jwk.algorithm) {
                (Some(KeyAlgorithm::ES256), _) => Algorithm::ES256,
                (Some(KeyAlgorithm::RS256), _) => Algorithm::RS256,
                (None, AlgorithmParameters::EllipticCurve(p)) if p.curve == EllipticCurve::P256 => {
                    Algorithm::ES256
                }
                (None, AlgorithmParameters::RSA(_)) => Algorithm::RS256,
                _ => continue,
            };
            if let Ok(key) = DecodingKey::from_jwk(jwk) {
                keys.insert(kid, (key, alg));
            }
        }
        Ok(keys)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use jsonwebtoken::{encode, EncodingKey, Header};
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    use serde_json::{json, Value};

    pub(crate) const SECRET: &[u8] = b"test-jwt-secret-0123456789abcdef";

    pub(crate) fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// An HS256 access token for `user`, as Supabase issues them.
    pub(crate) fn jwt(user: &str) -> String {
        sign_hs(json!({"sub": user, "aud": "authenticated", "exp": now() + 600, "role": "authenticated"}))
    }

    fn sign_hs(claims: Value) -> String {
        encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET)).unwrap()
    }

    fn hs_verifier() -> Verifier {
        Verifier::new(AuthConfig {
            hs256_secret: Some(SECRET.to_vec()),
            audience: "authenticated".into(),
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn hs256_tokens_name_their_user() {
        let v = hs_verifier();
        assert_eq!(v.verify(&jwt("user-1")).await.as_deref(), Ok("user-1"));
        assert_eq!(
            v.verify_header(Some(&format!("Bearer {}", jwt("user-1")))).await.as_deref(),
            Ok("user-1")
        );
        assert_eq!(v.verify_header(None).await, Err(AuthError::Missing));
        assert_eq!(v.verify_header(Some("Basic abc")).await, Err(AuthError::Missing));
    }

    #[tokio::test]
    async fn bad_tokens_are_refused() {
        let v = hs_verifier();
        let exp = now() + 600;
        let wrong_secret = encode(
            &Header::default(),
            &json!({"sub": "u", "aud": "authenticated", "exp": exp}),
            &EncodingKey::from_secret(b"another-secret"),
        )
        .unwrap();
        let mut tampered = jwt("user-1");
        tampered.insert(tampered.find('.').unwrap() + 2, 'x');
        for token in [
            sign_hs(json!({"sub": "u", "aud": "authenticated", "exp": now() - 120})),
            sign_hs(json!({"sub": "u", "aud": "anon", "exp": exp})),
            sign_hs(json!({"sub": "u", "exp": exp})),
            sign_hs(json!({"aud": "authenticated", "exp": exp, "role": "service_role"})),
            sign_hs(json!({"sub": "u", "aud": "authenticated"})),
            sign_hs(json!({"sub": "", "aud": "authenticated", "exp": exp})),
            wrong_secret,
            tampered,
            "not-a-jwt".into(),
            // `alg: none`.
            format!(
                "{}.{}.",
                URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#),
                URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"u","aud":"authenticated","exp":{exp}}}"#))
            ),
        ] {
            assert_eq!(v.verify(&token).await, Err(AuthError::Invalid), "{token}");
        }
    }

    #[test]
    fn some_verification_is_required() {
        assert!(Verifier::new(AuthConfig { audience: "authenticated".into(), ..Default::default() }).is_err());
    }

    fn sign_es(pair_pkcs8: &[u8], kid: &str, claims: Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.into());
        encode(&header, &claims, &EncodingKey::from_ec_der(pair_pkcs8)).unwrap()
    }

    #[tokio::test]
    async fn jwks_tokens_verify_and_unknown_keys_refresh_sparingly() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let point = pair.public_key().as_ref().to_vec();
        let set = json!({"keys": [{
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": "key-1",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        }]});
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let app = axum::Router::new().route(
            "/auth/v1/.well-known/jwks.json",
            axum::routing::get(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let set = set.clone();
                async move { axum::Json(set) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let v = Verifier::new(AuthConfig::supabase(&url, None, "authenticated".into())).unwrap();
        let issuer = format!("{url}/auth/v1");
        let claims = |iss: &str| json!({"sub": "user-es", "aud": "authenticated", "exp": now() + 600, "iss": iss});
        let hits_now = || hits.load(std::sync::atomic::Ordering::SeqCst);

        assert_eq!(v.verify(&sign_es(pkcs8.as_ref(), "key-1", claims(&issuer))).await.as_deref(), Ok("user-es"));
        assert_eq!(v.verify(&sign_es(pkcs8.as_ref(), "key-1", claims(&issuer))).await.as_deref(), Ok("user-es"));
        assert_eq!(hits_now(), 1, "keys are cached");

        // Wrong issuer; HS256 with no secret configured.
        assert_eq!(
            v.verify(&sign_es(pkcs8.as_ref(), "key-1", claims("https://evil/auth/v1"))).await,
            Err(AuthError::Invalid)
        );
        assert_eq!(v.verify(&jwt("user-1")).await, Err(AuthError::Invalid));

        // Unknown key ids do not turn into a fetch per token.
        let other = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        for _ in 0..3 {
            assert_eq!(
                v.verify(&sign_es(other.as_ref(), "key-2", claims(&issuer))).await,
                Err(AuthError::Invalid)
            );
        }
        assert_eq!(hits_now(), 1);
        // A known kid signed by the wrong key.
        assert_eq!(
            v.verify(&sign_es(other.as_ref(), "key-1", claims(&issuer))).await,
            Err(AuthError::Invalid)
        );
    }
}
