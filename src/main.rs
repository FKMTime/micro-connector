use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::OnceCell;
use tracing::info;

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

    //let test = r#"{"type":"IncidentResolved","data":{"attempt":{"id":"667fca70-f7f8-4e42-901a-a7829552a051","sessionId":"cde33ed3-dc20-469f-85ff-79db403c0abe","resultId":"a7e427bd-553a-4ae1-a85b-269584347597","attemptNumber":1,"replacedBy":null,"comment":null,"type":"STANDARD_ATTEMPT","status":"RESOLVED","penalty":2,"value":49,"inspectionTime":0,"judgeId":null,"deviceId":"b7dabdd9-6434-43fb-b7f4-339013bde15b","solvedAt":"2024-04-25T15:13:59.000Z","createdAt":"2024-04-25T15:14:00.154Z","updatedAt":"2024-04-25T15:15:19.675Z","device":{"id":"b7dabdd9-6434-43fb-b7f4-339013bde15b","name":"wqcewq","espId":648914624,"type":"STATION","batteryPercentage":58,"roomId":"4f74b618-007e-4c8a-8919-025def66b5cd","createdAt":"2024-04-24T18:25:29.579Z","updatedAt":"2024-04-25T15:15:10.804Z"},"result":{"id":"a7e427bd-553a-4ae1-a85b-269584347597","personId":"fa55a25a-ce7f-4926-91a1-450cd1798b03","eventId":"333","roundId":"333-r1","createdAt":"2024-04-25T15:14:00.149Z","updatedAt":"2024-04-25T15:14:00.149Z","person":{"id":"fa55a25a-ce7f-4926-91a1-450cd1798b03","registrantId":2,"name":"Maksymilian Gala","wcaId":"2022GALA01","countryIso2":"PL","gender":"m","canCompete":true,"birthdate":null,"giftpackCollectedAt":null,"cardId":"3004425529"}}},"espId":648914624,"shouldScanCards":true}}"#;
    //let test = r#"{"type":"IncidentResolved","data":{"espId":321,"shouldScanCards":true,"attempt":{"sessionId":"ds","penalty":-1,"value":69420,"inspectionTime":0}}}"#;
    let test = r#"{"type":"IncidentResolved","data":{"espId":321,"shouldScanCards":true, "attempt": { "sessionId": "cxz", "penalty": -1, "value": 0, "vcxvcxvxvcxdsadsadsatrewtwreterwtrwete": 321 }}}"#;
    let parsed: socket::structs::UnixResponse = serde_json::from_str(test)?;

    tracing::info!("parsed: {parsed:?}");

    return Ok(());

    _ = DEV_MODE.set(std::env::var("DEV").is_ok());
    let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
    _ = NEW_BUILD_BROADCAST.set(tx.clone());

    let (tx2, _) = tokio::sync::broadcast::channel::<()>(1);
    _ = REFRESH_DEVICE_SETTINGS_BROADCAST.set(tx2.clone());

    let comp_status = structs::SharedCompetitionStatus::new(tokio::sync::RwLock::new(
        structs::CompetitionStatus {
            should_update: false,
            devices_settings: HashMap::new(),
            broadcaster: tx2,
        },
    ));

    let socket_path = env_or_default("SOCKET_PATH", "/tmp/socket.sock");
    UNIX_SOCKET.init(&socket_path, comp_status.clone()).await?;

    bluetooth::start_bluetooth_task().await?;
    watchers::spawn_watchers(tx, comp_status.clone()).await?;
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
