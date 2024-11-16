use crate::handler::handle_client;
use crate::structs::SharedAppState;
use anyhow::Result;
use axum::extract::ws::WebSocket;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Router;
use axum::{extract::WebSocketUpgrade, routing::get};
use serde::Deserialize;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::{error, info};

fn default_chip() -> String {
    "no-chip".to_string()
}

fn default_firmware() -> String {
    "no-firmware".to_string()
}

#[derive(Debug, Deserialize)]
pub struct EspConnectInfo {
    pub id: u32,

    #[serde(rename = "ver")]
    pub version: String,

    #[serde(default = "default_chip")]
    pub chip: String,

    #[serde(default = "default_firmware")]
    pub firmware: String,
}

pub async fn start_server(port: u16, state: SharedAppState) -> Result<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!("Server started, listening on 0.0.0.0:{port}");

    let app = Router::new()
        .route("/", get(ws_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
        .with_state(state);

    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(esp_connect_info): Query<EspConnectInfo>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, esp_connect_info, state))
}

async fn handle_socket(socket: WebSocket, esp_connect_info: EspConnectInfo, state: SharedAppState) {
    info!("Client connected: {esp_connect_info:?}");

    let res = handle_client(socket, &esp_connect_info, state).await;
    if let Err(e) = res {
        error!("Handle client error: {e}");
    }

    info!("Client disconnected: {esp_connect_info:?}");
}
