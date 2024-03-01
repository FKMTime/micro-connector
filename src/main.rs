use anyhow::Result;
use std::sync::atomic::AtomicBool;
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
pub static SHOULD_UPDATE: AtomicBool = AtomicBool::new(true);

pub fn get_should_update() -> bool {
    SHOULD_UPDATE.load(std::sync::atomic::Ordering::Relaxed)
}

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    let api_url = std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:5000".to_string());
    API_URL.set(api_url)?;

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
