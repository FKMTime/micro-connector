use anyhow::Result;
use tracing::info;

use crate::structs::{TestPacketData, TimerPacket};

mod bluetooth;
mod github;
mod handler;
mod http;
mod mdns;
mod socket;
mod structs;
mod updater;
mod watchers;

//pub static DEV_MODE: OnceCell<bool> = OnceCell::const_new(); // TODO: Move to state
pub static UNIX_SOCKET: socket::Socket = socket::Socket::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    mdns::register_mdns(&port)?;

    //_ = DEV_MODE.set(std::env::var("DEV").is_ok());

    let state = structs::SharedAppState::new().await;

    let socket_path = env_or_default("SOCKET_PATH", "/tmp/socket.sock");
    UNIX_SOCKET.init(&socket_path, state.clone()).await?;

    bluetooth::start_bluetooth_task().await?;
    watchers::spawn_watchers(state.clone()).await?;
    tokio::task::spawn(http::start_server(port, state));

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM, stopping server!");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received SIGINT, stopping server!");
        }
    }

    Ok(())
}

fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
