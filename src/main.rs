use std::{os::unix::fs::PermissionsExt, path::PathBuf};

use anyhow::Result;

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
    //tracing_subscriber::fmt::init();
    log_subscriber::MinimalTracer::register()?;

    let device_id = 5436;
    tracing::info!(target = device_id, "123");
    tracing::info!(target = device_id, "1234");
    tracing::info!(target = device_id, "1235");
    tracing::info!(target = device_id, "1236");
    tracing::info!(target = device_id, "1237");
    tracing::info!(target = device_id, "1238");
    tracing::info!(target = device_id, "1239");
    tracing::info!(target = device_id, "12310");
    tracing::info!(target = device_id, "12311");
    tracing::info!(target = device_id, "12312");
    tracing::info!(target = device_id, "12313");
    tracing::info!(target = device_id, "12314");

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    mdns::register_mdns(&port)?;

    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);
    if !firmware_dir.exists() {
        tokio::fs::create_dir_all(&firmware_dir).await?;
        let mut perms = tokio::fs::metadata(&firmware_dir).await?.permissions();
        perms.set_mode(0o777);
    }

    let dev_mode = std::env::var("DEV").is_ok();
    let device_logs_path = std::env::var("DEVICE_LOGS").unwrap_or("/tmp/fkm-logs".to_string());
    let state = structs::SharedAppState::new(dev_mode, PathBuf::from(device_logs_path)).await;

    let socket_path = env_or_default("SOCKET_PATH", "/tmp/socket.sock");
    UNIX_SOCKET.init(&socket_path, state.clone()).await?;

    bluetooth::start_bluetooth_task().await?;
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
