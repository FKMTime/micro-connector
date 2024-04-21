use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::OnceCell;
use tracing::info;

mod api;
mod bluetooth;
mod github;
mod handler;
mod http;
mod mdns;
mod socket;
mod structs;
mod updater;
mod watchers;

pub static NEW_BUILD_BROADCAST: OnceCell<tokio::sync::broadcast::Sender<()>> =
    OnceCell::const_new();
pub static REFRESH_DEVICE_SETTINGS_BROADCAST: OnceCell<tokio::sync::broadcast::Sender<()>> =
    OnceCell::const_new();

pub static DEV_MODE: OnceCell<bool> = OnceCell::const_new();
pub static UNIX_SOCKET: socket::Socket = socket::Socket::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    mdns::register_mdns(&port)?;

    let socket_path = env_or_default("SOCKET_PATH", "/tmp/socket.sock");
    let api_url = env_or_default("API_URL", "http://localhost:5000");
    let api_token = env_or_err("API_TOKEN")?;
    UNIX_SOCKET.init(&socket_path).await?;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent("FKM-Timer/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    api::ApiClient::set_api_client(client, api_url, api_token)?;
    _ = DEV_MODE.set(std::env::var("DEV").is_ok());

    let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
    _ = NEW_BUILD_BROADCAST.set(tx.clone());

    let (tx2, _) = tokio::sync::broadcast::channel::<()>(1);
    _ = REFRESH_DEVICE_SETTINGS_BROADCAST.set(tx2.clone());

    let comp_status = structs::SharedCompetitionStatus::new(tokio::sync::RwLock::new(
        structs::CompetitionStatus {
            should_update: false,
            devices_settings: HashMap::new(),
        },
    ));

    bluetooth::start_bluetooth_task().await?;
    watchers::spawn_watchers(tx, tx2, comp_status.clone()).await?;
    tokio::task::spawn(http::start_server(port, comp_status.clone()));

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

fn env_or_err(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!(format!("{key} not set!")))
}
