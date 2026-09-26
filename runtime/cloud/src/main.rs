//! `zwiim-cloud`: the Cloud service binary, configured from the environment
//! (see `zwiim_cloud::config` for every variable).
//!
//! `zwiim-cloud --check` validates the configuration, prints its summary and
//! exits without listening. On SIGTERM or Ctrl-C the Cloud stops accepting,
//! tells hosts it is restarting (close 1012) and drains for a few seconds.

use zwiim_cloud::config::{LogFormat, Settings};
use zwiim_cloud::serve;

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("zwiim-cloud: {message}");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let settings = Settings::from_env(|name| std::env::var(name).ok()).unwrap_or_else(|e| fail(e));
    if std::env::args().skip(1).any(|a| a == "--check") {
        println!("zwiim-cloud: configuration OK: {}", settings.summary());
        return;
    }

    let filter = tracing_subscriber::EnvFilter::try_new(&settings.log_filter)
        .unwrap_or_else(|e| fail(format!("log filter {:?}: {e}", settings.log_filter)));
    let logs = tracing_subscriber::fmt().with_env_filter(filter);
    match settings.log_format {
        LogFormat::Text => logs.init(),
        LogFormat::Json => logs.json().init(),
    }

    let tls = settings
        .tls
        .as_ref()
        .map(|(cert, key)| serve::tls_config_from_files(cert, key).unwrap_or_else(|e| fail(e)));
    let listener = tokio::net::TcpListener::bind(settings.addr)
        .await
        .unwrap_or_else(|e| fail(format!("bind {}: {e}", settings.addr)));
    let running = serve::start(settings.config(), listener, tls).unwrap_or_else(|e| fail(e));
    tracing::info!(addr = %running.addr, "[cloud] listening: {}", settings.summary());

    shutdown_signal().await;
    tracing::info!("[cloud] shutting down");
    running.stop().await;
    tracing::info!("[cloud] stopped");
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .unwrap_or_else(|e| fail(e));
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
