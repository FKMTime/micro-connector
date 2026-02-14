use crate::handler::handle_client;
use crate::structs::SharedAppState;
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use anyhow::Result;
use axum::Router;
use axum::extract::ws::WebSocket;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::{extract::WebSocketUpgrade, routing::get};
use axum_server::tls_rustls::RustlsConfig;
use rcgen::CertifiedKey;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::{error, info};

fn default_firmware() -> String {
    "no-firmware".to_string()
}

fn default_random() -> u64 {
    0
}

#[derive(Debug, Deserialize)]
pub struct EspConnectInfo {
    pub id: u32,

    #[serde(rename = "ver")]
    pub version: String,

    #[serde(default = "default_firmware")]
    pub firmware: String,

    pub hw: String,

    #[serde(default = "default_random")]
    pub random: u64,
}

fn cert_from_str(cert: &str) -> Result<Vec<CertificateDer<'static>>> {
    rustls_pemfile::certs(&mut cert.as_bytes())
        .collect::<std::io::Result<_>>()
        .map_err(anyhow::Error::from)
}

fn key_from_str(key: &str) -> Result<PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut key.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("Private ket returned None"))
}

pub async fn start_server(port: u16, state: SharedAppState) -> Result<()> {
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    info!("Server started, listening on {addr}");

    let app = Router::new()
        .route("/", get(ws_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
        .with_state(state);

    if std::env::var("NO_TLS").is_ok() {
        let listener = TcpListener::bind(addr).await?;
        axum::serve(listener, app.into_make_service()).await?;
    } else {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Ring default provider install error");

        let CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["micro-connector.local".to_string()])?;
        let crt = cert_from_str(&cert.pem())?;
        let key = key_from_str(&signing_key.serialize_pem())?;

        let mut config = rustls::server::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(crt, key)?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        let config = RustlsConfig::from_config(Arc::new(config));
        axum_server::bind_rustls(addr, config)
            .serve(app.into_make_service())
            .await?;
    }
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(esp_connect_info): Query<EspConnectInfo>,
    State(state): State<SharedAppState>,
) -> impl IntoResponse {
    let mut headers = HeaderMap::new();

    let inner = state.inner.read().await;
    if let Some(device_settings) = inner.devices_settings.get(&esp_connect_info.id)
        && let Some(sign_key) = device_settings.sign_key
    {
        let mut key = [0; 16];
        key[..4].copy_from_slice(&sign_key.to_be_bytes());
        let key = GenericArray::from(key);

        let mut block = [0; 16];
        block[..8].copy_from_slice(&esp_connect_info.random.to_be_bytes());
        block[8..12].copy_from_slice(&inner.fkm_token.to_be_bytes());
        let mut block = GenericArray::from(block);

        let cipher = Aes128::new(&key);
        cipher.encrypt_block(&mut block);
        headers.insert(
            "RandomSigned",
            u128::from_be_bytes(block.into())
                .to_string()
                .parse()
                .expect(""),
        );
    }
    drop(inner);

    (
        headers,
        ws.on_upgrade(move |socket| handle_socket(socket, esp_connect_info, state)),
    )
}

async fn handle_socket(socket: WebSocket, esp_connect_info: EspConnectInfo, state: SharedAppState) {
    info!("Client connected: {esp_connect_info:?}");

    let res = handle_client(socket, &esp_connect_info, state).await;
    if let Err(e) = res {
        error!("Handle client error: {e}");
    }

    info!("Client disconnected: {esp_connect_info:?}");
    tracing::info!(
        file = format!("device_{:X}", esp_connect_info.id),
        "============= Client disconnected! ============="
    );
}
