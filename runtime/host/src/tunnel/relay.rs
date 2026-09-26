//! The relay transport: a Cloud relay WebSocket as the byte stream the
//! Noise tunnel runs over.
//!
//! ```text
//! noise::connect / noise::accept ⇄ DuplexStream ⇄ pump ⇄ WSS binary messages ⇄ Cloud
//! ```
//!
//! The tunnel itself is unchanged: this only moves its (already encrypted)
//! bytes in WebSocket binary messages. The same adapter serves both ends —
//! the host answering a dial and a device dialing in.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Largest message this end sends; the Cloud accepts up to
/// `zwiim_cloud::proto::MAX_RELAY_MESSAGE`.
pub const MAX_SEND: usize = 64 * 1024;
const PIPE_BYTES: usize = 256 * 1024;
/// Once one direction has ended, how long the other gets to finish.
const LINGER: Duration = Duration::from_secs(5);

/// Carry a byte stream over `ws`, whose relay handshake is complete. The
/// stream ends when the WebSocket closes; dropping or shutting down the
/// stream closes the WebSocket.
pub fn stream<S>(ws: WebSocketStream<S>) -> DuplexStream
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (ours, theirs) = tokio::io::duplex(PIPE_BYTES);
    tokio::spawn(pump(ws, theirs));
    ours
}

async fn pump<S>(ws: WebSocketStream<S>, io: DuplexStream)
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (mut from_tunnel, mut to_tunnel) = tokio::io::split(io);
    let (mut sink, mut source) = ws.split();
    let outbound = async move {
        let mut buf = vec![0u8; MAX_SEND];
        loop {
            match from_tunnel.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sink.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    };
    let inbound = async move {
        while let Some(msg) = source.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if to_tunnel.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                // Text is not part of the relay once it is open; a close or
                // an error ends it.
                Ok(Message::Text(_) | Message::Close(_)) | Err(_) => break,
            }
        }
        let _ = to_tunnel.shutdown().await;
    };
    tokio::pin!(outbound, inbound);
    tokio::select! {
        _ = &mut inbound => { let _ = tokio::time::timeout(LINGER, outbound).await; }
        _ = &mut outbound => { let _ = tokio::time::timeout(LINGER, inbound).await; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Role;

    async fn pair() -> (DuplexStream, DuplexStream) {
        let (a, b) = tokio::io::duplex(1 << 20);
        let a = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let b = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        (stream(a), stream(b))
    }

    #[tokio::test]
    async fn bytes_cross_in_both_directions_and_eof_propagates() {
        let (mut device, mut host) = pair().await;
        let big: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        let writer = {
            let big = big.clone();
            tokio::spawn(async move {
                device.write_all(&big).await.unwrap();
                device
            })
        };
        let mut got = vec![0u8; big.len()];
        host.read_exact(&mut got).await.unwrap();
        assert_eq!(got, big);
        let mut device = writer.await.unwrap();

        host.write_all(b"pong").await.unwrap();
        let mut back = [0u8; 4];
        device.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"pong");

        // One end closing is an EOF at the other.
        drop(device);
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), host.read_to_end(&mut rest))
            .await
            .unwrap()
            .unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn a_text_message_ends_the_stream() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let mut raw = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
        let mut io = stream(WebSocketStream::from_raw_socket(b, Role::Server, None).await);
        raw.send(Message::Binary(b"ok".to_vec().into())).await.unwrap();
        raw.send(Message::Text("{}".into())).await.unwrap();
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), io.read_to_end(&mut got))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, b"ok");
    }
}
