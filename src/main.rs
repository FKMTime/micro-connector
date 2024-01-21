use std::collections::HashMap;

use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
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
    version: &str,
) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);

    // TMP HASHMAP, TODO: other backend
    let mut cards_hashmap: HashMap<u128, (String, bool)> = HashMap::new();
    cards_hashmap.insert(3004425529, ("Filip Sciurka".to_string(), false));
    cards_hashmap.insert(2156233370, ("Filip Dziurka".to_string(), true));

    let firmware_file = tokio::fs::read("/tmp/firmware.bin").await?;
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

    let chunk_size = 4096;
    let mut firmware_chunks = firmware_file.chunks(chunk_size);

    while let Some(chunk) = firmware_chunks.next() {
        let frame = fastwebsockets::Frame::binary(chunk.into());
        ws.write_frame(frame).await?;

        // 250ms delay to allow esp to process
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    // test update process

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

    let version = query_map
        .get("ver")
        .expect("No version in query")
        .to_owned();

    tokio::task::spawn(async move {
        if let Err(e) = tokio::task::unconstrained(handle_client(fut, id, &version)).await {
            eprintln!("Error in websocket connection: {}", e);
        }

        println!("Client disconnected");
    });

    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), WebSocketError> {
    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    println!("Server started, listening on {}", "0.0.0.0:8080");

    loop {
        let (stream, _) = listener.accept().await?;
        println!("Client connected");
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
