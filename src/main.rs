use anyhow::Result;
use fastwebsockets::upgrade;
use fastwebsockets::WebSocketError;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use std::collections::HashMap;
use tokio::net::TcpListener;
use tokio::sync::OnceCell;

mod api;
mod handler;
mod mdns;
mod structs;
mod updater;

pub static NEW_BUILD_BROADCAST: OnceCell<tokio::sync::broadcast::Sender<()>> =
    OnceCell::const_new();
pub static API_URL: OnceCell<String> = OnceCell::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()?;
    let api_url = std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:5000".to_string());
    API_URL.set(api_url)?;

    mdns::register_mdns(&port)?;
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("Server started, listening on 0.0.0.0:{port}");

    let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
    NEW_BUILD_BROADCAST.set(tx.clone())?;
    updater::spawn_build_watcher(tx).await?;
    updater::spawn_github_releases_watcher().await?;

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            let conn_fut = http1::Builder::new()
                .serve_connection(io, service_fn(server_upgrade))
                .with_upgrades();

            if let Err(e) = conn_fut.await {
                println!("An error occurred: {:?}", e);
            }
        });
    }
}

async fn server_upgrade(
    mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>, WebSocketError> {
    let (response, fut) = upgrade::upgrade(&mut req)?;
    let query_map: HashMap<String, String> = req
        .uri()
        .query()
        .map(|q| {
            q.split('&')
                .map(|s| {
                    let mut split = s.split('=');
                    (
                        split.next().unwrap().to_string(),
                        split.next().unwrap().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let id = query_map
        .get("id")
        .expect("No id in query")
        .parse::<u128>()
        .unwrap();

    let build_time = query_map.get("bt").unwrap_or(&"0".to_string()).to_owned();
    let build_time = u128::from_str_radix(&build_time, 16).unwrap();

    let version = query_map
        .get("ver")
        .unwrap_or(&"NONE".to_string())
        .to_owned();

    let chip = query_map
        .get("chip")
        .unwrap_or(&"no-chip".to_string())
        .to_owned();

    println!(
        "Client connected: {} {} {} ({})",
        id, version, chip, build_time
    );
    tokio::task::spawn(async move {
        if let Err(e) =
            tokio::task::unconstrained(handler::handle_client(fut, id, &version, build_time, &chip))
                .await
        {
            eprintln!("Error in websocket connection: {}", e);
        }

        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        println!("Client disconnected ({})", epoch);
    });

    Ok(response)
}
