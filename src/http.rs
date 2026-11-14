use crate::handler::handle_client;
use crate::structs::SharedAppState;
use aes::Aes128;
use aes::cipher::{Array, BlockCipherEncrypt, KeyInit};
use anyhow::Result;
use axum::Router;
use axum::extract::ws::WebSocket;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{Extensions, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{extract::WebSocketUpgrade, routing::get};
use axum_server::tls_rustls::RustlsConfig;
use rcgen::CertifiedKey;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
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

#[derive(Debug, Clone, Deserialize)]
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

impl core::fmt::Display for EspConnectInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "EspConnectInfo {{ id: {:08X}, version: \"{}\", firmware: \"{}\", hw: \"{}\", random: {} }}",
            self.id, self.version, self.firmware, self.hw, self.random
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
        .ok_or_else(|| anyhow::anyhow!("Private ket returned None"))
}

pub fn router(state: SharedAppState) -> Router {
    Router::new()
        .route("/", get(ws_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
        .with_state(state)
}

pub async fn start_server(port: u16, state: SharedAppState) -> Result<()> {
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    info!("Server started, listening on {addr}");

    let app = router(state);

    if std::env::var("NO_TLS").is_ok() {
        let listener = TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
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
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    }
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(esp_connect_info): Query<EspConnectInfo>,
    State(state): State<SharedAppState>,
    extensions: Extensions,
) -> Response {
    // missing only when served without connect info (tests); the station link reader needs it
    let peer = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_canonical());

    let mut headers = HeaderMap::new();

    let inner = state.inner.read().await;
    // the device checks trust only in the handshake, so it has to wait for the sign keys; it
    // treats a non-101 reply as a connection error and retries
    if !inner.server_status_received {
        drop(inner);
        info!("Rejecting {esp_connect_info}: no ServerStatus from the backend yet");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    if let Some(device_settings) = inner.devices_settings.get(&esp_connect_info.id)
        && let Some(sign_key) = device_settings.sign_key
    {
        let mut key = [0; 16];
        key[..4].copy_from_slice(&sign_key.to_be_bytes());
        let key = Array::from(key);

        let mut block = [0; 16];
        block[..8].copy_from_slice(&esp_connect_info.random.to_be_bytes());
        block[8..12].copy_from_slice(&inner.fkm_token.to_be_bytes());
        let mut block = Array::from(block);

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
        ws.on_upgrade(move |socket| handle_socket(socket, esp_connect_info, peer, state)),
    )
        .into_response()
}

async fn handle_socket(
    socket: WebSocket,
    esp_connect_info: EspConnectInfo,
    peer: Option<IpAddr>,
    state: SharedAppState,
) {
    info!("Client connected: {esp_connect_info}");
    let _connected = state.connected_devices.track(esp_connect_info.id, peer);

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

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockCipherDecrypt;
    use std::time::Duration;
    use tokio::{
        io::AsyncWriteExt,
        net::{UnixListener, UnixStream},
        time::timeout,
    };
    use tokio_tungstenite::{connect_async, tungstenite};

    const KNOWN_DEVICE_ID: u32 = 0x1A2B3C4D;
    const UNKNOWN_DEVICE_ID: u32 = 0x5E6F7A8B;
    const SIGN_KEY: u32 = 123456;
    const FKM_TOKEN: i32 = 4242;
    const RANDOM: u64 = 987654321;
    const TIMEOUT: Duration = Duration::from_secs(2);

    type Handshake = Result<tungstenite::handshake::client::Response, tungstenite::Error>;

    /// A fake backend on its own unix socket; it stays bound so the connector can reconnect.
    struct Backend {
        listener: UnixListener,
        _requests: tokio::sync::mpsc::UnboundedSender<unix_utils::request::UnixRequest>,
    }

    async fn start_backend(state: &SharedAppState, name: &str) -> Backend {
        let path = std::env::temp_dir().join(format!("fkm-{name}-{}", std::process::id()));
        _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (requests, rx) = tokio::sync::mpsc::unbounded_channel();
        crate::socket::socket_task(path.to_str().unwrap().to_string(), rx, state.clone()).await;
        Backend {
            listener,
            _requests: requests,
        }
    }

    impl Backend {
        async fn accept(&self) -> UnixStream {
            let accept = timeout(TIMEOUT, self.listener.accept());
            accept.await.expect("connector didn't connect").unwrap().0
        }
    }

    async fn send_status(backend: &mut UnixStream, state: &SharedAppState) {
        let status = serde_json::json!({"type": "ServerStatus", "data": {
            "shouldUpdate": false, "devices": [{"espId": KNOWN_DEVICE_ID, "signKey": SIGN_KEY}],
            "translations": [], "defaultLocale": "en", "fkmToken": FKM_TOKEN,
            "secureRfid": false, "autoSetup": false, "soundEnabled": true,
        }});
        backend
            .write_all(status.to_string().as_bytes())
            .await
            .unwrap();
        backend.write_u8(0).await.unwrap();

        let received = async {
            while !state.inner.read().await.server_status_received {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        timeout(TIMEOUT, received)
            .await
            .expect("status not received");
    }

    async fn serve(state: &SharedAppState, id: u32) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move { axum::serve(listener, app).await });

        format!("ws://{addr}/?id={id}&ver=v3.4.4&hw=v4&firmware=STATION&random={RANDOM}")
    }

    async fn connect(state: &SharedAppState, id: u32) -> Handshake {
        let url = serve(state, id).await;
        connect_async(url).await.map(|(_, response)| response)
    }

    /// Decrypts `RandomSigned` the way the firmware does.
    fn random_signed(handshake: &Handshake) -> Option<(u64, i32)> {
        let header = handshake.as_ref().unwrap().headers().get("RandomSigned")?;
        let signed: u128 = header.to_str().unwrap().parse().unwrap();

        let mut key = [0; 16];
        key[..4].copy_from_slice(&SIGN_KEY.to_be_bytes());
        let mut block = Array::from(signed.to_be_bytes());
        Aes128::new(&Array::from(key)).decrypt_block(&mut block);

        let random = u64::from_be_bytes(block[..8].try_into().unwrap());
        let token = i32::from_be_bytes(block[8..12].try_into().unwrap());
        Some((random, token))
    }

    #[tokio::test]
    async fn upgrade_before_the_first_server_status_is_rejected() {
        let state = SharedAppState::new(false).await;
        let _backend = start_backend(&state, "no-status").await;

        match connect(&state, KNOWN_DEVICE_ID).await {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            }
            other => panic!("expected a 503, got {other:?}"),
        }
        assert!(!state.connected_devices.contains(KNOWN_DEVICE_ID));
    }

    #[tokio::test]
    async fn known_device_is_signed_after_the_first_server_status() {
        let state = SharedAppState::new(false).await;
        let backend = start_backend(&state, "known").await;
        send_status(&mut backend.accept().await, &state).await;

        let handshake = connect(&state, KNOWN_DEVICE_ID).await;
        assert_eq!(random_signed(&handshake), Some((RANDOM, FKM_TOKEN)));
    }

    // a device that isn't added yet still connects: it sets trust itself when submit is pressed
    #[tokio::test]
    async fn unknown_device_is_accepted_without_a_signature() {
        let state = SharedAppState::new(false).await;
        let backend = start_backend(&state, "unknown").await;
        send_status(&mut backend.accept().await, &state).await;

        let handshake = connect(&state, UNKNOWN_DEVICE_ID).await;
        assert!(handshake.is_ok());
        assert_eq!(random_signed(&handshake), None);
    }

    // the keys outlive the connection to the backend, so a backend restart locks nobody out
    #[tokio::test]
    async fn backend_reconnect_keeps_accepting_signed_devices() {
        let state = SharedAppState::new(false).await;
        let backend = start_backend(&state, "reconnect").await;
        send_status(&mut backend.accept().await, &state).await;

        // the first connection was dropped above; wait for the connector to come back
        let _reconnected = backend.accept().await;

        let handshake = connect(&state, KNOWN_DEVICE_ID).await;
        assert_eq!(random_signed(&handshake), Some((RANDOM, FKM_TOKEN)));
    }

    /// The station link reader finds the station's MAC through this address.
    #[tokio::test]
    async fn connected_device_is_tracked_with_its_address() {
        let state = SharedAppState::new(false).await;
        let backend = start_backend(&state, "peer").await;
        send_status(&mut backend.accept().await, &state).await;

        let url = serve(&state, KNOWN_DEVICE_ID).await;
        let _station = connect_async(url).await.unwrap();

        let tracked = async {
            while state.connected_devices.peers().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        timeout(TIMEOUT, tracked).await.expect("device not tracked");
        assert_eq!(
            state.connected_devices.peers(),
            [(KNOWN_DEVICE_ID, IpAddr::from([127, 0, 0, 1]))]
        );
    }
}
