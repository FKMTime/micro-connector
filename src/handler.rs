use crate::{
    http::EspConnectInfo,
    structs::{SharedAppState, TimerPacket},
};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use tracing::{error, info, trace};

pub async fn handle_client(
    mut socket: WebSocket,
    esp_connect_info: &EspConnectInfo,
    state: SharedAppState,
) -> Result<()> {
    {
        let state_inner = state.inner.read().await;
        if state_inner.should_update {
            if let Some(firmware) = super::updater::should_update(&state, esp_connect_info).await? {
                super::updater::update_client(&mut socket, &esp_connect_info, firmware).await?;

                return Ok(());
            }
        }
    }

    send_epoch_time(&mut socket).await?;
    send_device_status(&mut socket, esp_connect_info, &state).await?;
    let mut bc = state.get_bc().await;

    let interval_time = std::time::Duration::from_secs(5);
    let mut hb_interval = tokio::time::interval(interval_time);
    let mut hb_received = true;

    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                if !hb_received {
                    error!("Closing connection due to no heartbeat ({})", esp_connect_info.id);
                    break;
                }

                let msg = Message::Ping(vec![]);
                socket.send(msg).await?;
                hb_received = false;
            }
            Ok(res) = bc.recv() => {
                tracing::trace!("Bc recv: {:?}", res);

                match res {
                    crate::structs::BroadcastPacket::Build => {
                        let inner_state = state.inner.read().await;
                        if !inner_state.should_update {
                            continue;
                        }

                        let firmware = super::updater::should_update(&state, esp_connect_info).await?;
                        if let Some(firmware) = firmware {
                            let res = super::updater::update_client(&mut socket, esp_connect_info, firmware).await?;
                            if res {
                                break;
                            }
                        }
                    },
                    crate::structs::BroadcastPacket::Resp((esp_id, packet)) => {
                        if esp_connect_info.id == esp_id {
                            let resp = serde_json::to_string(&packet)?;
                            socket.send(Message::Text(resp)).await?;
                        }
                    },
                    crate::structs::BroadcastPacket::UpdateDeviceSettings => {
                        send_device_status(&mut socket, esp_connect_info, &state).await?;
                    }
                }
            }
            msg = socket.recv() => {
                let msg = msg.ok_or_else(|| anyhow::anyhow!("Frame option is null"))??;
                let res = on_ws_msg(&mut socket, msg, esp_connect_info, &mut hb_received).await;

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

async fn send_device_status(
    socket: &mut WebSocket,
    esp_connect_info: &EspConnectInfo,
    state: &SharedAppState,
) -> Result<()> {
    let state = state.inner.read().await;
    let settings = state.devices_settings.get(&esp_connect_info.id);
    let frame = if let Some(settings) = settings {
        TimerPacket::DeviceSettings {
            esp_id: esp_connect_info.id,
            use_inspection: settings.use_inspection,
            secondary_text: settings.secondary_text.clone(),
            added: true,
        }
    } else {
        TimerPacket::DeviceSettings {
            esp_id: esp_connect_info.id,
            use_inspection: None,
            secondary_text: None,
            added: false,
        }
    };

    let response = serde_json::to_string(&frame)?;
    socket.send(Message::Text(response)).await?;
    Ok(())
}

async fn send_epoch_time(socket: &mut WebSocket) -> Result<()> {
    let packet = TimerPacket::EpochTime {
        current_epoch: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
    };

    let resp = serde_json::to_string(&packet)?;
    socket.send(Message::Text(resp)).await?;
    Ok(())
}

async fn on_ws_msg(
    socket: &mut WebSocket,
    msg: Message,
    esp_connect_info: &EspConnectInfo,
    hb_received: &mut bool,
) -> Result<bool> {
    match msg {
        Message::Close(_) => {
            info!("Closing connection");
            return Ok(true);
        }
        Message::Pong(_) => {
            *hb_received = true;
        }
        Message::Text(payload) => {
            tracing::trace!("WS payload recv [{}]: {payload}", esp_connect_info.id);

            let response: TimerPacket = serde_json::from_str(&payload)?;
            let res = on_timer_response(socket, response).await;
            if let Err(e) = res {
                error!("on_timer_response error: {e:?}");
            }

            *hb_received = true;
        }

        _ => {}
    }

    Ok(false)
}

async fn on_timer_response(socket: &mut WebSocket, response: TimerPacket) -> Result<()> {
    match response {
        TimerPacket::CardInfoRequest {
            card_id,
            esp_id,
            attendance_device,
        } => {
            let attendance_device = attendance_device.unwrap_or(false);
            if attendance_device {
                _ = crate::socket::api::mark_attendance(esp_id, card_id).await;
                let resp = serde_json::to_string(&TimerPacket::AttendanceMarked { esp_id })?;
                socket.send(Message::Text(resp)).await?;

                return Ok(());
            }

            let response = match crate::socket::api::get_competitor_info(card_id).await {
                Ok(info) => {
                    trace!("Card info: {} {} {:?}", card_id, esp_id, info);
                    let response = TimerPacket::CardInfoResponse {
                        card_id,
                        esp_id,
                        country_iso2: info.country_iso2.unwrap_or_default(),
                        display: format!("{} ({})", info.name, info.registrant_id.unwrap_or(-1)),
                        can_compete: info.can_compete,
                    };

                    response
                }
                Err(e) => TimerPacket::ApiError {
                    esp_id,
                    error: e.message,
                    should_reset_time: e.should_reset_time,
                },
            };

            let response = serde_json::to_string(&response)?;
            socket.send(Message::Text(response)).await?;
        }
        TimerPacket::Solve {
            solve_time,
            penalty,
            competitor_id,
            judge_id,
            esp_id,
            timestamp,
            session_id,
            delegate,
            inspection_time,
        } => {
            trace!("Solve: {solve_time} ({penalty}) {competitor_id} {esp_id} {timestamp} {session_id} {delegate}");

            let res = crate::socket::api::send_solve_entry(
                solve_time,
                penalty,
                timestamp,
                esp_id,
                judge_id,
                competitor_id,
                delegate,
                &session_id,
                inspection_time,
            )
            .await;

            let resp = match res {
                Ok(_) => {
                    if delegate {
                        return Ok(());
                    }

                    TimerPacket::SolveConfirm {
                        esp_id,
                        session_id,
                        competitor_id,
                    }
                }
                Err(e) => TimerPacket::ApiError {
                    esp_id,
                    error: e.message,
                    should_reset_time: e.should_reset_time,
                },
            };

            let response = serde_json::to_string(&resp)?;
            socket.send(Message::Text(response)).await?;
        }
        TimerPacket::Logs { esp_id, logs } => {
            let mut msg_buf = String::new();
            for log in logs.iter().rev() {
                for line in log.msg.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    msg_buf.push_str(&format!("LOGS {} | {}\n", esp_id, line));
                }
            }

            info!("{}", msg_buf.trim());
        }
        TimerPacket::Battery {
            esp_id,
            level,
            voltage: _,
        } => {
            _ = crate::socket::api::send_battery_status(esp_id, level).await;
        }
        TimerPacket::Add { esp_id, firmware } => {
            _ = crate::socket::api::add_device(esp_id, &firmware).await;
            trace!("Add device: {}", esp_id);
        }
        TimerPacket::Snapshot(data) => {
            _ = crate::socket::api::send_snapshot_data(data).await;
        }
        TimerPacket::TestAck { esp_id } => {
            _ = crate::socket::api::send_test_ack(esp_id).await;
        }
        _ => {
            trace!("Not implemented timer response received: {:?}", response);
        }
    }

    Ok(())
}
