//! Noise IK handshake and the encrypted message layer.
//!
//! The device initiates with the host's static public key pinned at pairing
//! (IK: the responder's static key is known in advance); the host learns the
//! device's static key from the first message and decides whether to trust
//! it. Both static keys are the X25519 identity keys from pairing.
//!
//! The first message's payload is empty, except when a device pairs as it
//! connects: then it carries the pairing claim, which the host reads with the
//! device's key (see `super::host`).
//!
//! On the wire every Noise message is a big-endian `u16` length followed by
//! the message. A tunnel message larger than one Noise message (65535 bytes,
//! including the 16-byte tag) is split into chunks; each chunk's plaintext
//! starts with a flag byte, [`CHUNK_FINAL`] on the last one.

use std::future::Future;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use snow::{HandshakeState, TransportState};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};

pub const NOISE_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// Bound into the handshake with the host id, so a device only completes a
/// handshake with the host it paired with, on this protocol version.
const PROLOGUE: &[u8] = b"zwiim-tunnel/1";
const MAX_NOISE_MESSAGE: usize = 65535;
const TAG_LEN: usize = 16;
const MAX_CHUNK: usize = MAX_NOISE_MESSAGE - TAG_LEN - 1;
const CHUNK_MORE: u8 = 0;
const CHUNK_FINAL: u8 = 1;
/// Largest tunnel message, after reassembly.
pub const MAX_MESSAGE_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("tunnel I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The handshake did not complete: a wrong host key or host id, or a
    /// malformed message.
    #[error("tunnel handshake failed")]
    Handshake,
    /// The host completed the handshake but does not trust this device.
    #[error("the host refused this device")]
    Refused,
    /// A message failed authentication (tampered, replayed or out of order).
    #[error("tunnel message failed authentication")]
    Decrypt,
    #[error("tunnel message exceeds {MAX_MESSAGE_LEN} bytes")]
    TooLarge,
    #[error("malformed tunnel message")]
    Malformed,
    /// The peer closed the stream in the middle of a message.
    #[error("tunnel closed mid-message")]
    Truncated,
    #[error("tunnel closed")]
    Closed,
}

/// The host's verdict, the payload of the handshake's second message.
#[derive(Serialize, Deserialize)]
struct Verdict {
    ok: bool,
}

fn builder<'a>(host_id: &str, prologue: &'a mut Vec<u8>) -> snow::Builder<'a> {
    prologue.clear();
    prologue.extend_from_slice(PROLOGUE);
    prologue.push(0);
    prologue.extend_from_slice(host_id.as_bytes());
    snow::Builder::new(NOISE_PARAMS.parse().expect("valid Noise parameters"))
        .prologue(prologue)
        .expect("prologue set once")
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &[u8]) -> Result<(), TunnelError> {
    let len = u16::try_from(frame.len()).map_err(|_| TunnelError::TooLarge)?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(frame).await?;
    w.flush().await?;
    Ok(())
}

/// One frame, or `None` at a clean end of stream (between frames).
async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>, TunnelError> {
    let mut len = [0u8; 2];
    if r.read(&mut len[..1]).await? == 0 {
        return Ok(None);
    }
    r.read_exact(&mut len[1..]).await.map_err(eof_is_truncation)?;
    let len = u16::from_be_bytes(len) as usize;
    if len == 0 {
        return Err(TunnelError::Malformed);
    }
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame).await.map_err(eof_is_truncation)?;
    Ok(Some(frame))
}

fn eof_is_truncation(e: std::io::Error) -> TunnelError {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        TunnelError::Truncated
    } else {
        TunnelError::Io(e)
    }
}

/// Device end: handshake with the host whose static key the device pinned.
pub async fn connect<S>(
    io: S,
    host_id: &str,
    device_secret: &[u8; 32],
    host_static_pubkey: &[u8; 32],
) -> Result<Tunnel<S>, TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    connect_with(io, host_id, device_secret, host_static_pubkey, &[]).await
}

/// [`connect`], with `hello` as the first message's payload (encrypted to the
/// host's static key, and authenticated with the device's).
pub async fn connect_with<S>(
    mut io: S,
    host_id: &str,
    device_secret: &[u8; 32],
    host_static_pubkey: &[u8; 32],
    hello: &[u8],
) -> Result<Tunnel<S>, TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut prologue = Vec::new();
    let mut hs = builder(host_id, &mut prologue)
        .local_private_key(device_secret)
        .and_then(|b| b.remote_public_key(host_static_pubkey))
        .and_then(|b| b.build_initiator())
        .map_err(|_| TunnelError::Handshake)?;
    let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
    let n = hs.write_message(hello, &mut buf).map_err(|_| TunnelError::Handshake)?;
    write_frame(&mut io, &buf[..n]).await?;

    // A host that cannot read the first message (wrong key or host id) ends
    // the stream without answering.
    let reply = read_frame(&mut io).await?.ok_or(TunnelError::Handshake)?;
    let n = hs.read_message(&reply, &mut buf).map_err(|_| TunnelError::Handshake)?;
    let verdict: Verdict = serde_json::from_slice(&buf[..n]).map_err(|_| TunnelError::Handshake)?;
    if !verdict.ok {
        return Err(TunnelError::Refused);
    }
    Tunnel::new(io, hs)
}

/// Host end: complete the handshake if `authorize` trusts the device's static
/// public key (given the first message's payload too), returning what it
/// resolved the key to.
pub async fn accept<S, F, Fut, T>(
    mut io: S,
    host_id: &str,
    host_secret: &[u8; 32],
    authorize: F,
) -> Result<(Tunnel<S>, T), TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce([u8; 32], Vec<u8>) -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let mut prologue = Vec::new();
    let mut hs = builder(host_id, &mut prologue)
        .local_private_key(host_secret)
        .and_then(|b| b.build_responder())
        .map_err(|_| TunnelError::Handshake)?;
    let first = read_frame(&mut io).await?.ok_or(TunnelError::Closed)?;
    let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
    let n = hs.read_message(&first, &mut buf).map_err(|_| TunnelError::Handshake)?;
    let hello = buf[..n].to_vec();
    let device_key: [u8; 32] = hs
        .get_remote_static()
        .and_then(|k| k.try_into().ok())
        .ok_or(TunnelError::Handshake)?;

    let peer = authorize(device_key, hello).await;
    let verdict = serde_json::to_vec(&Verdict { ok: peer.is_some() }).expect("verdict serializes");
    let n = hs.write_message(&verdict, &mut buf).map_err(|_| TunnelError::Handshake)?;
    write_frame(&mut io, &buf[..n]).await?;
    match peer {
        Some(peer) => Ok((Tunnel::new(io, hs)?, peer)),
        None => {
            let _ = io.shutdown().await;
            Err(TunnelError::Refused)
        }
    }
}

/// An established tunnel.
pub struct Tunnel<S> {
    io: S,
    transport: TransportState,
}

impl<S: AsyncRead + AsyncWrite> Tunnel<S> {
    fn new(io: S, hs: HandshakeState) -> Result<Self, TunnelError> {
        Ok(Self {
            io,
            transport: hs.into_transport_mode().map_err(|_| TunnelError::Handshake)?,
        })
    }

    /// Separate halves, so one task can read while another writes.
    pub fn split(self) -> (TunnelReader<ReadHalf<S>>, TunnelWriter<WriteHalf<S>>) {
        let (r, w) = tokio::io::split(self.io);
        let transport = Arc::new(Mutex::new(self.transport));
        (
            TunnelReader {
                io: r,
                transport: transport.clone(),
                buf: vec![0u8; MAX_NOISE_MESSAGE],
            },
            TunnelWriter {
                io: w,
                transport,
                buf: vec![0u8; MAX_NOISE_MESSAGE],
            },
        )
    }
}

pub struct TunnelReader<R> {
    io: R,
    transport: Arc<Mutex<TransportState>>,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> TunnelReader<R> {
    /// The next message, or `None` when the peer closed the tunnel cleanly.
    pub async fn recv(&mut self) -> Result<Option<Vec<u8>>, TunnelError> {
        let mut message = Vec::new();
        loop {
            let Some(frame) = read_frame(&mut self.io).await? else {
                return if message.is_empty() {
                    Ok(None)
                } else {
                    Err(TunnelError::Truncated)
                };
            };
            let n = self
                .transport
                .lock()
                .unwrap()
                .read_message(&frame, &mut self.buf)
                .map_err(|_| TunnelError::Decrypt)?;
            let (&flag, chunk) = self.buf[..n].split_first().ok_or(TunnelError::Malformed)?;
            if message.len() + chunk.len() > MAX_MESSAGE_LEN {
                return Err(TunnelError::TooLarge);
            }
            message.extend_from_slice(chunk);
            match flag {
                CHUNK_FINAL => return Ok(Some(message)),
                CHUNK_MORE => {}
                _ => return Err(TunnelError::Malformed),
            }
        }
    }
}

pub struct TunnelWriter<W> {
    io: W,
    transport: Arc<Mutex<TransportState>>,
    buf: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> TunnelWriter<W> {
    pub async fn send(&mut self, message: &[u8]) -> Result<(), TunnelError> {
        if message.len() > MAX_MESSAGE_LEN {
            return Err(TunnelError::TooLarge);
        }
        let mut chunks = message.chunks(MAX_CHUNK).peekable();
        let mut plain = Vec::with_capacity(MAX_CHUNK + 1);
        loop {
            let chunk = chunks.next().unwrap_or_default();
            plain.clear();
            plain.push(if chunks.peek().is_some() { CHUNK_MORE } else { CHUNK_FINAL });
            plain.extend_from_slice(chunk);
            let n = self
                .transport
                .lock()
                .unwrap()
                .write_message(&plain, &mut self.buf)
                .map_err(|_| TunnelError::Malformed)?;
            write_frame(&mut self.io, &self.buf[..n]).await?;
            if chunks.peek().is_none() {
                return Ok(());
            }
        }
    }

    /// End the tunnel: the peer reads a clean close.
    pub async fn close(mut self) {
        let _ = self.io.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::DuplexStream;

    use super::*;

    const HOST_ID: &str = "host-1";

    fn keypair() -> ([u8; 32], [u8; 32]) {
        let k = snow::Builder::new(NOISE_PARAMS.parse().unwrap())
            .generate_keypair()
            .unwrap();
        (k.private.try_into().unwrap(), k.public.try_into().unwrap())
    }

    /// A handshake between a device and a host that trusts it.
    async fn tunnels() -> (Tunnel<DuplexStream>, Tunnel<DuplexStream>) {
        let (host_secret, host_public) = keypair();
        let (device_secret, device_public) = keypair();
        let (d, h) = tokio::io::duplex(1 << 20);
        let host = tokio::spawn(async move {
            accept(h, HOST_ID, &host_secret, |key, _| async move {
                (key == device_public).then_some("dev-1")
            })
            .await
        });
        let device = connect(d, HOST_ID, &device_secret, &host_public).await.unwrap();
        let (host, peer) = host.await.unwrap().unwrap();
        assert_eq!(peer, "dev-1");
        (device, host)
    }

    /// One final chunk carrying `message`, encrypted as `w` would send it.
    fn seal<W>(w: &mut TunnelWriter<W>, flag: u8, message: &[u8]) -> Vec<u8> {
        let plain = [&[flag][..], message].concat();
        let n = w.transport.lock().unwrap().write_message(&plain, &mut w.buf).unwrap();
        w.buf[..n].to_vec()
    }

    #[tokio::test]
    async fn messages_cross_both_ways_and_large_ones_are_chunked() {
        let (device, host) = tunnels().await;
        let (mut dr, mut dw) = device.split();
        let (mut hr, mut hw) = host.split();
        for len in [0, 1, MAX_CHUNK, MAX_CHUNK + 1, 3 * MAX_CHUNK + 5] {
            let message: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let (sent, received) = tokio::join!(dw.send(&message), hr.recv());
            sent.unwrap();
            assert_eq!(received.unwrap().unwrap(), message, "{len} bytes to the host");
            let (sent, received) = tokio::join!(hw.send(&message), dr.recv());
            sent.unwrap();
            assert_eq!(received.unwrap().unwrap(), message, "{len} bytes to the device");
        }
    }

    #[tokio::test]
    async fn oversized_messages_are_refused_both_ways() {
        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();
        assert!(matches!(
            dw.send(&vec![0; MAX_MESSAGE_LEN + 1]).await,
            Err(TunnelError::TooLarge)
        ));
        // A peer that keeps sending continuation chunks is cut off at the cap.
        let chunk = vec![0; MAX_CHUNK];
        let flood = async {
            for _ in 0..=MAX_MESSAGE_LEN / MAX_CHUNK {
                let frame = seal(&mut dw, CHUNK_MORE, &chunk);
                if write_frame(&mut dw.io, &frame).await.is_err() {
                    break;
                }
            }
        };
        let received = async {
            let received = hr.recv().await;
            drop(hr);
            received
        };
        let ((), received) = tokio::join!(flood, received);
        assert!(matches!(received, Err(TunnelError::TooLarge)), "{received:?}");
    }

    #[tokio::test]
    async fn tampered_and_replayed_messages_fail_authentication() {
        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();

        let frame = seal(&mut dw, CHUNK_FINAL, b"{\"t\":\"req\"}");
        write_frame(&mut dw.io, &frame).await.unwrap();
        assert_eq!(hr.recv().await.unwrap().unwrap(), b"{\"t\":\"req\"}");
        // The same ciphertext again: its nonce is spent.
        write_frame(&mut dw.io, &frame).await.unwrap();
        assert!(matches!(hr.recv().await, Err(TunnelError::Decrypt)));

        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();
        let mut frame = seal(&mut dw, CHUNK_FINAL, b"{\"t\":\"req\"}");
        frame[3] ^= 0x01;
        write_frame(&mut dw.io, &frame).await.unwrap();
        assert!(matches!(hr.recv().await, Err(TunnelError::Decrypt)));
    }

    #[tokio::test]
    async fn a_clean_close_is_told_apart_from_a_cut_or_malformed_stream() {
        let (device, host) = tunnels().await;
        let (_dr, dw) = device.split();
        let (mut hr, _hw) = host.split();
        dw.close().await;
        assert!(matches!(hr.recv().await, Ok(None)));

        // Cut inside a frame.
        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();
        dw.io.write_all(&100u16.to_be_bytes()).await.unwrap();
        dw.io.write_all(&[0; 10]).await.unwrap();
        dw.close().await;
        assert!(matches!(hr.recv().await, Err(TunnelError::Truncated)));

        // Cut between the chunks of one message.
        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();
        let frame = seal(&mut dw, CHUNK_MORE, b"half");
        write_frame(&mut dw.io, &frame).await.unwrap();
        dw.close().await;
        assert!(matches!(hr.recv().await, Err(TunnelError::Truncated)));

        // An empty frame, and an unknown chunk flag.
        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();
        dw.io.write_all(&0u16.to_be_bytes()).await.unwrap();
        assert!(matches!(hr.recv().await, Err(TunnelError::Malformed)));
        let (device, host) = tunnels().await;
        let (_dr, mut dw) = device.split();
        let (mut hr, _hw) = host.split();
        let frame = seal(&mut dw, 7, b"x");
        write_frame(&mut dw.io, &frame).await.unwrap();
        assert!(matches!(hr.recv().await, Err(TunnelError::Malformed)));
    }

    #[tokio::test]
    async fn the_handshake_binds_the_host_key_and_host_id_and_the_host_decides() {
        let (host_secret, host_public) = keypair();
        let (device_secret, _) = keypair();
        let (_, other_public) = keypair();
        let attempt = |host_id: &'static str, pinned: [u8; 32], trusted: bool| async move {
            let (d, h) = tokio::io::duplex(1 << 16);
            let host = tokio::spawn(async move {
                accept(h, HOST_ID, &host_secret, |_, _| async move { trusted.then_some(()) })
                    .await
                    .map(|_| ())
            });
            let device = connect(d, host_id, &device_secret, &pinned).await.map(|_| ());
            (device, host.await.unwrap())
        };

        let (device, host) = attempt(HOST_ID, host_public, true).await;
        assert!(device.is_ok() && host.is_ok());
        // A device pinned to another host key, or bound for another host id,
        // gets no answer.
        let (device, host) = attempt(HOST_ID, other_public, true).await;
        assert!(matches!(device, Err(TunnelError::Handshake)), "{device:?}");
        assert!(matches!(host, Err(TunnelError::Handshake)), "{host:?}");
        let (device, host) = attempt("host-2", host_public, true).await;
        assert!(matches!(device, Err(TunnelError::Handshake)), "{device:?}");
        assert!(matches!(host, Err(TunnelError::Handshake)), "{host:?}");
        // A host that does not trust the device refuses it.
        let (device, host) = attempt(HOST_ID, host_public, false).await;
        assert!(matches!(device, Err(TunnelError::Refused)), "{device:?}");
        assert!(matches!(host, Err(TunnelError::Refused)), "{host:?}");
    }

    #[tokio::test]
    async fn the_host_reads_the_devices_hello_with_its_key() {
        let (host_secret, host_public) = keypair();
        let (device_secret, device_public) = keypair();
        let (d, h) = tokio::io::duplex(1 << 16);
        let host = tokio::spawn(async move {
            accept(h, HOST_ID, &host_secret, |key, hello| async move { Some((key, hello)) })
                .await
                .map(|(_, peer)| peer)
        });
        connect_with(d, HOST_ID, &device_secret, &host_public, b"{\"pair\":1}").await.unwrap();
        assert_eq!(host.await.unwrap().unwrap(), (device_public, b"{\"pair\":1}".to_vec()));

        // A plain connect says nothing.
        let (d, h) = tokio::io::duplex(1 << 16);
        let host = tokio::spawn(async move {
            accept(h, HOST_ID, &host_secret, |_, hello| async move { Some(hello) })
                .await
                .map(|(_, hello)| hello)
        });
        connect(d, HOST_ID, &device_secret, &host_public).await.unwrap();
        assert!(host.await.unwrap().unwrap().is_empty());
    }
}
