//! Fake FKMTime server used by firmware device simulations.
use anyhow::{Context, Result};
use std::path::Path;

mod sim;

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();

    let socket_path = std::env::var("SOCKET_PATH").unwrap_or("/tmp/sock/socket.sock".to_string());
    if let Some(parent) = Path::new(&socket_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::remove_file(&socket_path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    let config = tokio::fs::read("tests.json")
        .await
        .context("Cannot read fake FKMTime configuration: tests.json")?;
    sim::run(&socket_path, &config).await
}
