//! Zwiim-owned identity (Identity V1 — MVP).
//!
//! ```text
//! Human
//!   └── Host                 host_id            (host.rs)
//!        └── trusted_devices device_id + status (trusted_devices.rs)
//! ```
//!
//! Identity is kept separate from authentication and from resource ids:
//!
//! - **Identity** — opaque `host_id` / `device_id`. Never mixed with
//!   `workspace_id` / `session_id` / `terminal_id`.
//! - **Authentication** — unchanged bearer mechanism (`web::auth`). The host's
//!   own access token keeps working for the local desktop; a paired device
//!   presents the per-device bearer credential it received at pairing. Only a
//!   SHA-256 of that credential is stored, and it is cleared on revocation.
//! - **Authorization** — binary: trusted or not trusted.
//! - **Pairing** — one-time, short-lived, in-memory invitation carried by a QR
//!   payload (pairing.rs). The QR is never a reusable credential.
//!
//! `device.rs` is the device-side half (a client's persistent `device_id` and
//! per-host credentials); it lives here until a client crate exists.
//!
//! Deferred to the security/identity hardening phase (not V1):
//! host/device keypairs (Ed25519), X25519, keychain/secure-enclave storage,
//! cryptographic QR payloads, challenge-response authentication (replacing the
//! per-device bearer credential), TLS/transport redesign, Cloud relay /
//! rendezvous, Google/Supabase human identity, multi-user partitioning /
//! `user_id` foreign keys, authorization scopes (today any trusted device may
//! also mint invitations and revoke devices), provider-secret migration, and
//! terminating an already-open WebSocket when its device is revoked (new
//! requests are rejected immediately; a live `/ws/events` stream ends on its
//! next reconnect).

pub mod device;
pub mod host;
pub mod pairing;
pub mod trusted_devices;
