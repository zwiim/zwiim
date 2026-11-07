//! Serving the Cloud: TLS (rustls) on any address, plain HTTP on loopback
//! only (development, tests, or behind a local TLS-terminating proxy).

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::hub::{client_key, PerMinute};
use crate::server::{router, Cloud, Config};

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const FULL_WARNING_EVERY: Duration = Duration::from_secs(60);

/// A rustls server config from PEM: a certificate chain and its key.
pub fn tls_config(cert_pem: &[u8], key_pem: &[u8]) -> Result<Arc<rustls::ServerConfig>, String> {
    let certs = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("TLS certificate: {e}"))?;
    if certs.is_empty() {
        return Err("TLS certificate: no certificate found".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| format!("TLS key: {e}"))?;
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub fn tls_config_from_files(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    tls_config(&read(cert)?, &read(key)?)
}

/// A running Cloud.
pub struct Running {
    pub addr: SocketAddr,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Running {
    /// Stop accepting, end every control and relay socket, and release the
    /// port.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

/// Start the Cloud on `listener`. Without TLS the listener must be loopback.
pub fn start(
    config: Config,
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<Running, String> {
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    if tls.is_none() && !addr.ip().is_loopback() {
        return Err("plain HTTP is only served on loopback; configure TLS".into());
    }
    let (stop, shutdown) = watch::channel(false);
    let slots = Arc::new(Semaphore::new(config.limits.max_connections));
    let clients = PerMinute::new(config.limits.max_connects_per_ip);
    let cloud = Cloud::new(config, shutdown.clone())?;
    let app = router(cloud);
    let tls = tls.map(TlsAcceptor::from);
    let task = tokio::spawn(accept_loop(listener, tls, app, slots, clients, shutdown));
    Ok(Running { addr, stop, task })
}

async fn accept_loop(
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    app: Router,
    slots: Arc<Semaphore>,
    clients: PerMinute<IpAddr>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connections = tokio::task::JoinSet::new();
    let mut last_full_warning: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = crate::hub::stopped(&mut shutdown) => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    // One client cannot use up the slots everyone shares.
                    if client_key(peer.ip()).is_some_and(|client| !clients.allow(client)) {
                        drop(stream);
                        continue;
                    }
                    // At the connection limit: close it at once.
                    let Ok(slot) = slots.clone().try_acquire_owned() else {
                        if last_full_warning.is_none_or(|t| t.elapsed() > FULL_WARNING_EVERY) {
                            last_full_warning = Some(Instant::now());
                            tracing::warn!("[cloud] at the connection limit; closing new connections");
                        }
                        drop(stream);
                        continue;
                    };
                    let _ = stream.set_nodelay(true);
                    let stream = Counted { io: stream, _slot: slot };
                    let (tls, app, shutdown) = (tls.clone(), app.clone(), shutdown.clone());
                    connections.spawn(async move {
                        match tls {
                            Some(acceptor) => {
                                if let Ok(Ok(stream)) =
                                    tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await
                                {
                                    connection(stream, app, shutdown).await;
                                }
                            }
                            None => connection(stream, app, shutdown).await,
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("[cloud] accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drop(listener);
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, async { while connections.join_next().await.is_some() {} }).await;
}

/// A connection holding its slot until the socket itself is dropped, which
/// for a WebSocket is after the upgrade, when the socket ends.
struct Counted<S> {
    io: S,
    _slot: OwnedSemaphorePermit,
}

impl<S: AsyncRead + Unpin> AsyncRead for Counted<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counted<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

async fn connection<S>(io: S, app: Router, mut shutdown: watch::Receiver<bool>)
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
    let conn = builder
        .serve_connection(TokioIo::new(io), TowerToHyperService::new(app))
        .with_upgrades();
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => {}
        _ = crate::hub::stopped(&mut shutdown) => {
            conn.as_mut().graceful_shutdown();
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, conn).await;
        }
    }
}
