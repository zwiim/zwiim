//! Zwiim-owned identity.
//!
//! ```text
//! Human
//!   └── Host                 host_id + host keys          (host.rs, host_keys.rs)
//!        └── trusted_devices device_id + device_pubkey    (trusted_devices.rs)
//!                            + status + last_seen_at
//! ```
//!
//! Identity is kept separate from authentication and from resource ids:
//!
//! - **Identity** — opaque `host_id` / `device_id`, never mixed with
//!   `workspace_id` / `session_id` / `terminal_id`. The host holds two
//!   keypairs with separate purposes (Ed25519 signing for the Cloud control
//!   plane, X25519 static for the device tunnel), kept in the OS keychain or,
//!   opted into for headless hosts, a private file (host_keys.rs). A device
//!   holds its own X25519 static keypair (device.rs).
//! - **Trust** — the host is the final authority. Key pairing registers the
//!   device's static public key; revocation is persistent and closes the
//!   device's open event sockets (`web::auth::device_revoked`).
//! - **Authentication** — `web::auth`. The host's own access token (compared
//!   in constant time) keeps working for the local desktop. Devices paired the
//!   legacy way present a per-device bearer credential (only its SHA-256 is
//!   stored, cleared on revocation); no new bearer credentials back the remote
//!   trust model, which authenticates the device's static key over the
//!   encrypted tunnel (`crate::tunnel`, which attaches `web::auth::TunnelPeer`).
//! - **Authorization** — two levels: local host authority (the host token)
//!   and a trusted device. Device management and `host.claim` require local
//!   host authority.
//! - **Pairing** — one-time, short-lived, in-memory invitations (pairing.rs).
//!   QR v2 carries the host's static public key and no network address; the
//!   QR is never a reusable credential.
//!
//! `device.rs` is the device-side half; it lives here until a client crate
//! exists.

pub mod device;
pub mod host;
pub mod host_keys;
pub mod pairing;
pub mod trusted_devices;
