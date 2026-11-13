//! Zwiim Cloud: managed connectivity between remote devices and hosts.
//!
//! ```text
//! device ──WSS── Cloud ──WSS── host
//!    └──────── Noise IK ────────┘      (the Cloud relays ciphertext)
//! ```
//!
//! - [`proto`] — the wire contract, shared with hosts and devices.
//! - `auth` — account tokens (Supabase JWTs) → user id.
//! - `config` — deployment settings from the environment, validated.
//! - `registry` — account ↔ host, public keys. Persistent.
//! - `hub` — enrollment tokens, tickets, presence, rendezvous. In memory.
//! - `server` — the endpoints, control socket and relay.
//! - `serve` — TLS and the listener.
//! - `usage` — relay bytes per account and channel, plan limits.
//!
//! The Cloud is never in a local execution path and never authoritative for
//! host state: a host keeps working with the Cloud gone.

pub mod proto;

#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod config;
#[cfg(feature = "server")]
pub mod hub;
#[cfg(feature = "server")]
pub mod registry;
#[cfg(feature = "server")]
pub mod serve;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod usage;

#[cfg(all(test, feature = "server"))]
mod tests;
