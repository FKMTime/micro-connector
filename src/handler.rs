use crate::{
    http::EspConnectInfo,
    structs::{SharedAppState, TimerPacket, TimerPacketInner},
};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use tracing::{error, info, trace};

pub async fn handle_client(
    mut socket: WebSocket,
    esp_connect_info: &EspConnectInfo,
    state: SharedAppState,
) -> Result<()> {
    tracing::info!(
        file = format!("device_{}", esp_connect_info.id),
        "============= Client connected! ============="
    );

    {
        let state_inner = state.inner.read().await;
        if state_inner.should_update {
            if let Some(firmware) = super::updater::should_update(&state, esp_connect_info).await? {
                tracing::info!(
                    file = format!("device_{}", esp_connect_info.id),
                    "Starting update."
                );
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
                    tracing::error!(file = format!("device_{}", esp_connect_info.id), "============= Closing connection (due to no heartbeat) =============");
                    break;
                }

                let msg = Message::Ping(vec![].into());
                socket.send(msg).await?;
                hb_received = false;
            }
            Ok(res) = bc.recv() => {
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
                            socket.send(Message::Text(resp.into())).await?;
                        }
                    },
                    crate::structs::BroadcastPacket::UpdateDeviceSettings => {
                        send_device_status(&mut socket, esp_connect_info, &state).await?;
                    }
                    crate::structs::BroadcastPacket::ForceUpdate((hw, firmware)) => {
                        if firmware.firmware == esp_connect_info.firmware && hw == esp_connect_info.hw {
                            let res = super::updater::update_client(&mut socket, esp_connect_info, firmware).await?;
                            if res {
                                break;
                            }
                        }
                    }
                }
            }
            msg = socket.recv() => {
                let msg = msg.ok_or_else(|| anyhow::anyhow!("Frame option is null"))??;
                let res = on_ws_msg(&mut socket, msg, esp_connect_info, &mut hb_received, &state).await;

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
    let settings_frame = if let Some(_settings) = settings {
        TimerPacket {
            tag: None,
            data: TimerPacketInner::DeviceSettings {
                added: true,
                locales: state.locales.clone(),
                default_locale: state.default_locale.clone(),
            },
        }
    } else {
        TimerPacket {
            tag: None,
            data: TimerPacketInner::DeviceSettings {
                added: false,
                locales: state.locales.clone(),
                default_locale: state.default_locale.clone(),
            },
        }
    };

    let response = serde_json::to_string(&settings_frame)?;
    socket.send(Message::Text(response.into())).await?;
    Ok(())
}

async fn send_epoch_time(socket: &mut WebSocket) -> Result<()> {
    let packet = TimerPacket {
        tag: None,
        data: TimerPacketInner::EpochTime {
            current_epoch: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        },
    };

    let resp = serde_json::to_string(&packet)?;
    socket.send(Message::Text(resp.into())).await?;
    Ok(())
}

async fn on_ws_msg(
    socket: &mut WebSocket,
    msg: Message,
    esp_connect_info: &EspConnectInfo,
    hb_received: &mut bool,
    state: &SharedAppState,
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
            let res = on_timer_response(socket, response, esp_connect_info, state).await;
            if let Err(e) = res {
                error!("on_timer_response error: {e:?}");
            }

            *hb_received = true;
        }

        _ => {}
    }

    Ok(false)
}

async fn on_timer_response(
    socket: &mut WebSocket,
    response: TimerPacket,
    esp_connect_info: &EspConnectInfo,
    state: &SharedAppState,
) -> Result<()> {
    let esp_id = esp_connect_info.id;

    match response.data {
        TimerPacketInner::CardInfoRequest {
            card_id,
            attendance_device,
        } => {
            let attendance_device = attendance_device.unwrap_or(false);
            if attendance_device {
                _ = crate::socket::api::mark_attendance(esp_id, card_id).await;
                let resp = serde_json::to_string(&TimerPacket {
                    tag: response.tag,
                    data: TimerPacketInner::AttendanceMarked,
                })?;
                socket.send(Message::Text(resp.into())).await?;

                return Ok(());
            }

            let response =
                match crate::socket::api::get_competitor_info(card_id, esp_connect_info.id).await {
                    Ok(info) => {
                        let registrant_display = match info.registrant_id {
                            Some(x) => format!(" ({x})"),
                            None => String::new(),
                        };

                        trace!("Card info: {} {} {:?}", card_id, esp_id, info);
                        let response = TimerPacket {
                            tag: response.tag,
                            data: TimerPacketInner::CardInfoResponse {
                                card_id,
                                country_iso2: info.country_iso2.unwrap_or_default(),
                                display: format!("{}{}", info.name, registrant_display),
                                can_compete: info.can_compete,
                                possible_groups: info.possible_groups,
                            },
                        };

                        response
                    }
                    Err(e) => TimerPacket {
                        tag: response.tag,
                        data: TimerPacketInner::ApiError {
                            error: e.message,
                            should_reset_time: e.should_reset_time,
                        },
                    },
                };

            let response = serde_json::to_string(&response)?;
            socket.send(Message::Text(response.into())).await?;
        }
        TimerPacketInner::Solve {
            solve_time,
            penalty,
            competitor_id,
            judge_id,
            timestamp,
            session_id,
            delegate,
            inspection_time,
            group_id,
        } => {
            trace!("Solve: {solve_time} ({penalty}) {competitor_id} {esp_id} {timestamp} {session_id} {delegate} {group_id}");

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
                &group_id,
            )
            .await;

            let resp = match res {
                Ok(_) => {
                    if delegate {
                        return Ok(());
                    }

                    TimerPacket {
                        tag: response.tag,
                        data: TimerPacketInner::SolveConfirm {
                            session_id,
                            competitor_id,
                        },
                    }
                }
                Err(e) => TimerPacket {
                    tag: response.tag,
                    data: TimerPacketInner::ApiError {
                        error: e.message,
                        should_reset_time: e.should_reset_time,
                    },
                },
            };

            let response = serde_json::to_string(&resp)?;
            socket.send(Message::Text(response.into())).await?;
        }
        TimerPacketInner::Logs { logs } => {
            for log in logs.iter().rev() {
                for line in log.lines() {
                    if line.is_empty() {
                        continue;
                    }

                    tracing::info!(file = format!("device_{esp_id}"), "{line}");
                }
            }
        }
        TimerPacketInner::Battery { level, voltage: _ } => {
            let inner_state = state.inner.read().await;
            if inner_state.devices_settings.contains_key(&esp_id) {
                _ = crate::socket::api::send_battery_status(esp_id, level).await;
            }
        }
        TimerPacketInner::Add { firmware } => {
            let inner_state = state.inner.read().await;
            if !inner_state.devices_settings.contains_key(&esp_id) {
                _ = crate::socket::api::add_device(esp_id, &firmware).await;
                trace!("Add device: {}", esp_id);
            }
        }
        TimerPacketInner::TestAck(snapshot) => {
            let inner_state = state.inner.read().await;
            if inner_state.devices_settings.contains_key(&esp_id) {
                _ = crate::socket::api::send_test_ack(esp_id, snapshot).await;
            }
        }
        _ => {
            trace!("Not implemented timer response received: {:?}", response);
        }
    }

    Ok(())
}
