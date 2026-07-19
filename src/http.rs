use crate::handler::handle_client;
use crate::structs::SharedAppState;
use anyhow::Result;
use axum::Router;
use axum::extract::ws::WebSocket;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{extract::WebSocketUpgrade, routing::get};
use axum_server::tls_rustls::RustlsConfig;
use rcgen::CertifiedKey;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::{error, info, warn};

fn default_firmware() -> String {
    "no-firmware".to_string()
}

#[derive(Debug, Deserialize)]
pub struct EspConnectInfo {
    pub id: u32,

    #[serde(rename = "ver")]
    pub version: String,

    #[serde(default = "default_firmware")]
    pub firmware: String,

    pub hw: String,
}

impl core::fmt::Display for EspConnectInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "EspConnectInfo {{ id: {:08X}, version: \"{}\", firmware: \"{}\", hw: \"{}\" }}",
            self.id, self.version, self.firmware, self.hw
        )
    }
}

fn cert_from_str(cert: &str) -> Result<Vec<CertificateDer<'static>>> {
    rustls_pemfile::certs(&mut cert.as_bytes())
        .collect::<std::io::Result<_>>()
        .map_err(anyhow::Error::from)
}

fn key_from_str(key: &str) -> Result<PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut key.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("Private key returned None"))
}

/// Directory for persistent TLS material. Override with TLS_CERT_DIR.
fn tls_cert_dir() -> PathBuf {
    std::env::var("TLS_CERT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./certs"))
}

fn load_or_generate_tls_material(dir: &Path) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read_to_string(&cert_path)?;
        let key_pem = std::fs::read_to_string(&key_path)?;
        info!("Loaded TLS cert from {}", cert_path.display());
        return Ok((cert_from_str(&cert_pem)?, key_from_str(&key_pem)?));
    }

    std::fs::create_dir_all(dir)?;
    let CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["micro-connector.local".to_string()])?;
    let cert_pem = cert.pem();
    let key_pem = signing_key.serialize_pem();
    std::fs::write(&cert_path, &cert_pem)?;
    std::fs::write(&key_path, &key_pem)?;
    // Best-effort restrictive perms on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::set_permissions(&cert_path, std::fs::Permissions::from_mode(0o644));
    }
    info!("Generated new TLS cert at {}", cert_path.display());

    // Log fingerprint for operator / device pin debugging
    if let Ok(certs) = cert_from_str(&cert_pem)
        && let Some(c) = certs.first()
    {
        let dig = ring::digest::digest(&ring::digest::SHA256, c.as_ref());
        info!(
            "TLS cert SHA-256 fingerprint: {}",
            hex::encode(dig.as_ref())
        );
    }

    Ok((cert_from_str(&cert_pem)?, key_from_str(&key_pem)?))
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
        warn!(
            "NO_TLS is set — stations cannot pin a certificate. Use only for local e2e/debug."
        );
        let listener = TcpListener::bind(addr).await?;
        axum::serve(listener, app.into_make_service()).await?;
    } else {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Ring default provider install error");

        let dir = tls_cert_dir();
        let (crt, key) = load_or_generate_tls_material(&dir)?;

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
    // No RandomSigned header — trust is TLS pin + connect-time HMAC.
    ws.on_upgrade(move |socket| handle_socket(socket, esp_connect_info, state))
}

async fn handle_socket(socket: WebSocket, esp_connect_info: EspConnectInfo, state: SharedAppState) {
    info!("Client connected: {esp_connect_info}");

    let res = handle_client(socket, &esp_connect_info, state).await;
    if let Err(e) = res {
        error!("Handle client error: {e}");
    }

    info!("Client disconnected: {esp_connect_info}");
    tracing::info!(
        file = format!("device_{:X}", esp_connect_info.id),
        "============= Client disconnected! ============="
    );
}
