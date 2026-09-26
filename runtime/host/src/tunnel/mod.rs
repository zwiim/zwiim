//! The device tunnel: the Zwiim protocol carried over a Noise-encrypted,
//! mutually authenticated byte stream.
//!
//! ```text
//! device ── Noise IK (device static key ⇄ host static key) ── host adapter
//!                                                              └─► /zwiim/v1 router
//! ```
//!
//! The tunnel is transport only. Operations and the event socket keep their
//! `/zwiim/v1` contract: a `req` is the operation's POST, a `ws.*` stream is
//! the `/zwiim/v1/ws` socket (see [`envelope`]).
//!
//! - [`noise`] — handshake, framing, chunking. Shared by both ends.
//! - [`envelope`] — the messages inside the tunnel. Shared by both ends.
//! - [`client`] — the device end: request/response correlation and event
//!   streams. Used by tests and the `tunnel_client` example.
//! - [`host`] — the host end: authenticates the device key against the
//!   trusted-device registry and routes into the protocol router in-process.
//! - [`relay`] — the tunnel's byte stream carried over a Cloud relay
//!   WebSocket. Shared by both ends.
//!
//! The shared parts depend on nothing else in this crate, so a client crate
//! can take them over when one exists. Remote devices reach the host through
//! the Cloud relay (`crate::cloud`); the host also accepts tunnels on an
//! opt-in loopback listener for local verification
//! ([`host::serve_test_listener`]).

pub mod client;
pub mod envelope;
pub mod host;
pub mod noise;
pub mod relay;

#[cfg(test)]
pub(crate) mod tests;
