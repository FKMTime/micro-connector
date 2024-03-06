use anyhow::Result;
use std::sync::{Arc, RwLock};
use structs::UpdateStrategy;
use tokio::sync::OnceCell;
use tracing::info;

mod api;
mod handler;
mod http;
mod mdns;
mod structs;
mod updater;

pub static NEW_BUILD_BROADCAST: OnceCell<tokio::sync::broadcast::Sender<()>> =
    OnceCell::const_new();
pub static UPDATE_STRATEGY: OnceCell<Arc<RwLock<UpdateStrategy>>> = OnceCell::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();
    _ = UPDATE_STRATEGY.set(Arc::new(RwLock::new(UpdateStrategy::Disabled)));

    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    mdns::register_mdns(&port)?;

    let api_url = std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:5000".to_string());
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent("FKM-Timer/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    api::ApiClient::set_api_client(client, api_url)?;

    let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
    _ = NEW_BUILD_BROADCAST.set(tx.clone());

    updater::spawn_watchers(tx).await?;
    tokio::task::spawn(http::start_server(port));

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
