use std::collections::HashMap;

use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use hyper::Body;
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
    Connect {
        esp_id: u128,
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

async fn handle_client(fut: upgrade::UpgradeFut) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);

    // TMP HASHMAP, TODO: other backend
    let mut cards_hashmap: HashMap<u128, (String, bool)> = HashMap::new();
    cards_hashmap.insert(3004425529, ("Filip Sciurka".to_string(), false));
    cards_hashmap.insert(2156233370, ("Filip Dziurka".to_string(), true));

    loop {
        let frame = ws.read_frame().await?;
        match frame.opcode {
            OpCode::Close => break,
            OpCode::Text | OpCode::Binary => {
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

    Ok(())
}
async fn server_upgrade(mut req: Request<Body>) -> Result<Response<Body>, WebSocketError> {
    let (response, fut) = upgrade::upgrade(&mut req)?;

    tokio::task::spawn(async move {
        if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
            eprintln!("Error in websocket connection: {}", e);
        }

        println!("Client disconnected (2)");
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
            let conn_fut = Http::new()
                .serve_connection(stream, service_fn(server_upgrade))
                .with_upgrades();
            if let Err(e) = conn_fut.await {
                println!("An error occurred: {:?}", e);
            }

            println!("Client disconnected");
        });
    }
}
