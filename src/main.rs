use anyhow::Result;
use std::sync::{Arc, RwLock};
use structs::UpdateStrategy;
use tokio::sync::OnceCell;

mod api;
mod handler;
mod http;
mod mdns;
mod structs;
mod updater;

pub static NEW_BUILD_BROADCAST: OnceCell<tokio::sync::broadcast::Sender<()>> =
    OnceCell::const_new();
pub static API_URL: OnceCell<String> = OnceCell::const_new();
pub static UPDATE_STRATEGY: OnceCell<Arc<RwLock<UpdateStrategy>>> = OnceCell::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    let api_url = std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:5000".to_string());
    API_URL.set(api_url)?;
    _ = UPDATE_STRATEGY.set(Arc::new(RwLock::new(UpdateStrategy::Disabled)));

    mdns::register_mdns(&port)?;

    let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
    NEW_BUILD_BROADCAST.set(tx.clone())?;
    updater::spawn_build_watcher(tx).await?;
    updater::spawn_github_releases_watcher().await?;
    updater::spawn_should_update_status_watcher().await?;
    tokio::task::spawn(http::start_server(port));

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = sigterm.recv() => {
            println!("Received SIGTERM, stopping server!");
        }
        _ = tokio::signal::ctrl_c() => {
            println!("Received SIGINT, stopping server!");
        }
    }

    Ok(())
}
