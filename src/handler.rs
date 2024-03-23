use crate::{
    http::EspConnectInfo,
    structs::{SharedCompetitionStatus, TimerResponse},
};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use tracing::{error, info, trace};

pub async fn handle_client(
    mut socket: WebSocket,
    esp_connect_info: &EspConnectInfo,
    comp_status: SharedCompetitionStatus,
) -> Result<()> {
    if comp_status.read().await.should_update
        && super::updater::update_client(&mut socket, esp_connect_info).await?
    {
        return Ok(());
    }
    send_device_status(&mut socket, esp_connect_info, &comp_status).await?;

    let mut update_broadcast = super::NEW_BUILD_BROADCAST
        .get()
        .expect("build broadcast channel not set")
        .subscribe();

    let mut update_device_settings_broadcast = super::REFRESH_DEVICE_SETTINGS_BROADCAST
        .get()
        .expect("device settings broadcast channel not set")
        .subscribe();

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
            _ = update_broadcast.recv() => {
                if !comp_status.read().await.should_update {
                    continue;
                }

                let res = super::updater::update_client(&mut socket, esp_connect_info).await?;
                if res {
                    break;
                }
            }
            _ = update_device_settings_broadcast.recv() => {
                send_device_status(&mut socket, esp_connect_info, &comp_status).await?;
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
    comp_status: &SharedCompetitionStatus,
) -> Result<()> {
    let comp_status = comp_status.read().await;
    let settings = comp_status.devices_settings.get(&esp_connect_info.id);
    if let Some(settings) = settings {
        let frame = TimerResponse::DeviceSettings {
            esp_id: esp_connect_info.id,
            use_inspection: settings.use_inspection,
        };

        let response = serde_json::to_string(&frame)?;
        socket.send(Message::Text(response)).await?;
    }

    Ok(())
}

async fn on_ws_msg(
    socket: &mut WebSocket,
    msg: Message,
    _esp_connect_info: &EspConnectInfo,
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
            let response: TimerResponse = serde_json::from_str(&payload)?;
            let res = on_timer_response(socket, response).await;
            if let Err(e) = res {
                error!("on_timer_response error: {e:?}");
            }
        }

        _ => {}
    }

    Ok(false)
}

async fn on_timer_response(socket: &mut WebSocket, response: TimerResponse) -> Result<()> {
    let (client, api_url) = crate::api::ApiClient::get_api_client()?;

    match response {
        TimerResponse::CardInfoRequest { card_id, esp_id } => {
            let response = match crate::api::get_competitor_info(&client, &api_url, card_id).await {
                Ok(info) => {
                    trace!("Card info: {} {} {:?}", card_id, esp_id, info);
                    let response = TimerResponse::CardInfoResponse {
                        card_id,
                        esp_id,
                        country_iso2: info.country_iso2.unwrap_or_default(),
                        display: format!("{} ({})", info.name, info.registrant_id.unwrap_or(-1)),
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

            let response = serde_json::to_string(&response)?;
            socket.send(Message::Text(response)).await?;
        }
        TimerResponse::Solve {
            solve_time,
            penalty,
            competitor_id: solver_id,
            judge_id,
            esp_id,
            timestamp,
            session_id,
            delegate,
        } => {
            trace!("Solve: {solve_time} ({penalty}) {solver_id} {esp_id} {timestamp} {session_id} {delegate}");

            let res = crate::api::send_solve_entry(
                &client,
                &api_url,
                solve_time,
                penalty,
                timestamp,
                esp_id,
                judge_id,
                solver_id,
                delegate,
                &session_id,
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

            let response = serde_json::to_string(&resp)?;
            socket.send(Message::Text(response)).await?;
        }
        TimerResponse::Logs { esp_id, logs } => {
            let mut msg_buf = String::new();
            for log in logs.iter().rev() {
                for line in log.msg.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    msg_buf.push_str(&format!("{} | {}\n", esp_id, line));
                }
            }

            info!("LOGS:\n{}", msg_buf);
        }
        TimerResponse::Battery {
            esp_id,
            level,
            voltage,
        } => {
            crate::api::send_battery_status(&client, &api_url, esp_id, level).await?;
            trace!("Battery: {} {} {}", esp_id, level, voltage);
        }
        _ => {
            trace!("Not implemented timer response received: {:?}", response);
        }
    }

    Ok(())
}
