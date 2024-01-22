use crate::structs::TimerResponse;
use fastwebsockets::{OpCode, WebSocketError};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;

pub async fn handle_client(
    fut: fastwebsockets::upgrade::UpgradeFut,
    id: u128,
    version_time: u128,
    chip: &str,
) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);
    if super::updater::update_client(&mut ws, id, version_time, chip).await? {
        return Ok(());
    }

    let mut update_broadcast = super::NEW_BUILD_BROADCAST
        .get()
        .unwrap()
        .clone()
        .subscribe();

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
            _ = update_broadcast.recv() => {
                let res = super::updater::update_client(&mut ws, id, version_time, chip).await?;
                if res {
                    break;
                }
            }
            frame = ws.read_frame() => {
                let frame = frame?;
                on_ws_frame(&mut ws, id, version_time, chip, frame, &mut hb_recieved, &cards_hashmap).await?;
            }
        }
    }

    Ok(())
}

async fn on_ws_frame(
    ws: &mut fastwebsockets::FragmentCollector<TokioIo<Upgraded>>,
    _id: u128,
    _version_time: u128,
    _chip: &str,
    frame: fastwebsockets::Frame<'_>,
    hb_recieved: &mut bool,
    cards_hashmap: &HashMap<u128, (String, bool)>,
) -> Result<(), WebSocketError> {
    match frame.opcode {
        OpCode::Close => {
            println!("Closing connection");
            return Err(WebSocketError::ConnectionClosed);
        }
        OpCode::Pong => {
            *hb_recieved = true;
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

    Ok(())
}
