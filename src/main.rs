use std::collections::HashMap;
use std::path::PathBuf;

use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct LogData {
    pub millis: u128,
    pub msg: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
enum TimerResponse {
    StartUpdate {
        esp_id: u128,
        version: String,
        size: i64,
    },
    Solve {
        solve_time: u128,
        card_id: u128,
        esp_id: u128,
        timestamp: u128,
        session_id: i64,
    },
    SolveConfirm {
        esp_id: u128,
        card_id: u128,
        session_id: i64,
    },
    CardInfoRequest {
        card_id: u128,
        esp_id: u128,
    },
    CardInfoResponse {
        card_id: u128,
        esp_id: u128,
        name: String,
        is_judge: bool,
    },
    Logs {
        esp_id: u128,
        logs: Vec<LogData>,
    },
}

async fn handle_client(
    fut: upgrade::UpgradeFut,
    id: u128,
    version_time: u128,
    chip: &str,
) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);
    if update_client(&mut ws, id, version_time, chip).await? {
        return Ok(());
    }

    // TMP HASHMAP, TODO: other backend
    let mut cards_hashmap: HashMap<u128, (String, bool)> = HashMap::new();
    cards_hashmap.insert(3004425529, ("Filip Sciurka".to_string(), false));
    cards_hashmap.insert(2156233370, ("Filip Dziurka".to_string(), true));

    let interval_time = std::time::Duration::from_secs(5);
    let mut hb_interval = tokio::time::interval(interval_time);
    let mut hb_recieved = true;

    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                if !hb_recieved {
                    println!("Closing connection due to no heartbeat");
                    break;
                }

                let frame = fastwebsockets::Frame::new(true, OpCode::Ping, None, vec![].into());
                ws.write_frame(frame).await?;
                hb_recieved = false;
            }
            frame = ws.read_frame() => {
                let frame = frame?;

                match frame.opcode {
                    OpCode::Close => break,
                    OpCode::Pong => {
                        hb_recieved = true;
                    }
                    OpCode::Text => {
                        let response: TimerResponse = serde_json::from_slice(&frame.payload).unwrap();
                        match response {
                            TimerResponse::CardInfoRequest { card_id, esp_id } => {
                                if let Some(name) = cards_hashmap.get(&card_id) {
                                    let response = TimerResponse::CardInfoResponse {
                                        card_id,
                                        esp_id,
                                        name: name.0.to_string(),
                                        is_judge: name.1,
                                    };

                                    let response = serde_json::to_vec(&response).unwrap();
                                    let frame = fastwebsockets::Frame::text(response.into());
                                    ws.write_frame(frame).await?;
                                }
                            }
                            TimerResponse::Solve {
                                solve_time,
                                card_id,
                                esp_id,
                                timestamp,
                                session_id,
                            } => {
                                println!(
                                    "Solve: {} {} {} {} {}",
                                    solve_time, card_id, esp_id, timestamp, session_id
                                );

                                let response = TimerResponse::SolveConfirm {
                                    esp_id,
                                    session_id,
                                    card_id,
                                };
                                let response = serde_json::to_vec(&response).unwrap();
                                let frame = fastwebsockets::Frame::text(response.into());
                                ws.write_frame(frame).await?;
                            }
                            _ => {
                                println!("Received: {:?}", response);
                                ws.write_frame(frame).await?;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

/// Returns true if client was updated
async fn update_client(
    ws: &mut fastwebsockets::FragmentCollector<TokioIo<Upgraded>>,
    id: u128,
    version_time: u128,
    chip: &str,
) -> Result<bool, WebSocketError> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_firmware: (Option<PathBuf>, u128) = (None, version_time);
    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_split: Vec<&str> = file_name.to_str().unwrap().split('.').collect();

        if name_split.len() < 3 || name_split[0] != chip {
            continue;
        }

        let version = u128::from_str_radix(name_split[1], 16).unwrap();
        if version > latest_firmware.1 {
            latest_firmware = (Some(entry.path()), version);
        }
    }

    if latest_firmware.0.is_none() || version_time >= latest_firmware.1 {
        return Ok(false);
    }

    println!("Updating client to version {}", latest_firmware.1);
    println!("Firmware file: {:?}", latest_firmware.0);
    let firmware_file = tokio::fs::read(latest_firmware.0.unwrap()).await?;
    let frame = fastwebsockets::Frame::text(
        serde_json::to_vec(&TimerResponse::StartUpdate {
            esp_id: id,
            version: "new".to_string(),
            size: firmware_file.len() as i64,
        })
        .unwrap()
        .into(),
    );
    ws.write_frame(frame).await?;

    // 1s delay to allow esp to process
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let chunk_size = 1024 * 4;
    let mut firmware_chunks = firmware_file.chunks(chunk_size);

    while let Some(chunk) = firmware_chunks.next() {
        let frame = fastwebsockets::Frame::binary(chunk.into());
        ws.write_frame(frame).await?;

        if firmware_chunks.len() % 10 == 0 {
            println!(
                "[{}] {}/{} chunks left",
                id,
                firmware_chunks.len(),
                firmware_file.len() / chunk_size
            );
        }

        if firmware_chunks.len() == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Wait for esp to process
                                                                         // (important)

            break;
        }

        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), ws.read_frame())
            .await
            .or_else(|_| {
                println!("Timeout while updating");
                Err(WebSocketError::ConnectionClosed)
            })?;

        let frame = frame?;
        if frame.opcode == OpCode::Close {
            return Ok(false);
        }
    }

    Ok(true)
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

    let version = query_map.get("ver").unwrap_or(&"0".to_string()).to_owned();
    let version_time = u128::from_str_radix(&version, 16).unwrap();

    let chip = query_map
        .get("chip")
        .unwrap_or(&"no-chip".to_string())
        .to_owned();

    println!("Client connected: {} {} {}", id, version, chip);
    tokio::task::spawn(async move {
        if let Err(e) =
            tokio::task::unconstrained(handle_client(fut, id, version_time, &chip)).await
        {
            eprintln!("Error in websocket connection: {}", e);
        }

        println!("Client disconnected");
    });

    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), WebSocketError> {
    _ = dotenvy::dotenv();
    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    println!("Server started, listening on {}", "0.0.0.0:8080");

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
