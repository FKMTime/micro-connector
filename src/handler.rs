use crate::structs::{TimerResponse, UpdateStrategy};
use anyhow::Result;
use base64::prelude::*;
use fastwebsockets::{OpCode, WebSocketError};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use std::io::{BufRead, Write};
use tracing::{debug, error, info, trace};

pub async fn handle_client(
    fut: fastwebsockets::upgrade::UpgradeFut,
    id: u128,
    version: &str,
    build_time: u128,
    chip: &str,
) -> Result<(), WebSocketError> {
    let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);
    if UpdateStrategy::should_update()
        && super::updater::update_client(&mut ws, id, version, build_time, chip).await?
    {
        return Ok(());
    }

    let mut update_broadcast = super::NEW_BUILD_BROADCAST
        .get()
        .unwrap()
        .clone()
        .subscribe();

    let interval_time = std::time::Duration::from_secs(5);
    let mut hb_interval = tokio::time::interval(interval_time);
    let mut hb_recieved = true;

    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                if !hb_recieved {
                    error!("Closing connection due to no heartbeat ({id})");
                    break;
                }

                let frame = fastwebsockets::Frame::new(true, OpCode::Ping, None, vec![].into());
                ws.write_frame(frame).await?;
                hb_recieved = false;
            }
            _ = update_broadcast.recv() => {
                if !UpdateStrategy::should_update() {
                    continue;
                }

                let res = super::updater::update_client(&mut ws, id, version, build_time, chip).await?;
                if res {
                    break;
                }
            }
            frame = ws.read_frame() => {
                let frame = frame?;
                let res = on_ws_frame(&mut ws, id, build_time, chip, frame, &mut hb_recieved).await;

                match res {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => {
                        error!("Ws read frame error: {}", e);
                    }
                }
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
) -> Result<bool> {
    match frame.opcode {
        OpCode::Close => {
            info!("Closing connection");
            return Ok(true);
        }
        OpCode::Pong => {
            *hb_recieved = true;
        }
        OpCode::Text => {
            let response: TimerResponse = serde_json::from_slice(&frame.payload).unwrap();
            match response {
                TimerResponse::CardInfoRequest { card_id, esp_id } => {
                    let response = match crate::api::get_competitor_info(card_id).await {
                        Ok(info) => {
                            trace!("Card info: {} {} {:?}", card_id, esp_id, info);
                            let response = TimerResponse::CardInfoResponse {
                                card_id,
                                esp_id,
                                country_iso2: info.country_iso2.unwrap_or_default(),
                                display: format!(
                                    "{} ID: {}",
                                    info.name,
                                    info.registrant_id.unwrap_or(-1)
                                ),
                                can_compete: info.can_compete,
                            };

                            response
                        }
                        Err(e) => TimerResponse::ApiError {
                            esp_id,
                            error: e.message,
                            should_reset_time: e.should_reset_time,
                        },
                    };

                    let response = serde_json::to_vec(&response).unwrap();
                    let frame = fastwebsockets::Frame::text(response.into());
                    ws.write_frame(frame).await?;
                }
                TimerResponse::Solve {
                    solve_time,
                    offset,
                    competitor_id: solver_id,
                    judge_id,
                    esp_id,
                    timestamp,
                    session_id,
                    delegate,
                } => {
                    trace!(
                        "Solve: {solve_time} ({offset}) {solver_id} {esp_id} {timestamp} {session_id} {delegate}",
                    );

                    let res = crate::api::send_solve_entry(
                        solve_time, offset, timestamp, esp_id, judge_id, solver_id, delegate,
                    )
                    .await;

                    let resp = match res {
                        Ok(_) => TimerResponse::SolveConfirm {
                            esp_id,
                            session_id,
                            competitor_id: solver_id,
                        },
                        Err(e) => TimerResponse::ApiError {
                            esp_id,
                            error: e.message,
                            should_reset_time: e.should_reset_time,
                        },
                    };

                    let response = serde_json::to_vec(&resp).unwrap();
                    let frame = fastwebsockets::Frame::text(response.into());
                    ws.write_frame(frame).await?;
                }
                TimerResponse::Logs { esp_id, logs } => {
                    let mut msg_buf = String::new();
                    for log in logs.iter().rev() {
                        let msg = BASE64_STANDARD.decode(&log.msg.as_bytes()).unwrap();
                        for line in msg.lines() {
                            let line = line?;
                            if line.is_empty() {
                                continue;
                            }
                            msg_buf.push_str(&format!("{} | {}\n", esp_id, line));
                        }
                    }

                    info!("LOGS:\n{}", msg_buf);
                }
                _ => {
                    trace!("Not implemented timer response received: {:?}", response);
                    ws.write_frame(frame).await?;
                }
            }
        }
        _ => {}
    }

    Ok(false)
}
