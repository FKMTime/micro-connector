use anyhow::Result;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

mod bluetooth;
mod github;
mod handler;
mod http;
mod log_subscriber;
mod mdns;
mod socket;
mod structs;
mod updater;
mod watchers;

pub static UNIX_SOCKET: socket::Socket = socket::Socket::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();

    let logs_path = env_or_default("DEVICE_LOGS", "/tmp/fkm-logs");
    log_subscriber::MinimalTracer::register(PathBuf::from(logs_path))?;

    let firmware_dir = env_or_default("FIRMWARE_DIR", "/tmp/fkm-build");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);
    if !firmware_dir.exists() {
        tokio::fs::create_dir_all(&firmware_dir).await?;
        let mut perms = tokio::fs::metadata(&firmware_dir).await?.permissions();
        perms.set_mode(0o777);
    }

    let dev_mode = std::env::var("DEV").is_ok();
    let state = structs::SharedAppState::new(dev_mode).await;

    let socket_path = env_or_default("SOCKET_PATH", "/tmp/socket.sock");
    let port: u16 = env_or_default("PORT", "8080").parse()?;
    UNIX_SOCKET.init(&socket_path, state.clone()).await?;

    if std::env::var("NO_MDNS").is_err() {
        mdns::register_mdns(&port)?;
    }

    if std::env::var("NO_BT").is_err() {
        bluetooth::start_bluetooth_task().await?;
    }

    watchers::spawn_watchers(state.clone()).await?;
    tokio::task::spawn(http::start_server(port, state));

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, stopping server!");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, stopping server!");
        }
    }

    Ok(())
}

fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
