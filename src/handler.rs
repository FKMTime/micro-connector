use crate::{
    http::EspConnectInfo,
    log_packet::LogPacket,
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
        file = format!("device_{:X}", esp_connect_info.id),
        "============= Client connected! ============="
    );

    // never hold a state guard across an update: it parks ServerStatus' write, and every new
    // connection behind it, until the update ends
    let updates_allowed = state.inner.read().await.should_update;
    if updates_allowed
        && let Some(firmware) = super::updater::should_update(&state, esp_connect_info).await?
    {
        tracing::info!(
            file = format!("device_{:X}", esp_connect_info.id),
            "Starting update."
        );
        super::updater::update_client(&mut socket, esp_connect_info, firmware).await?;

        return Ok(());
    }

    send_epoch_time(&mut socket).await?;
    send_device_status(&mut socket, esp_connect_info, &state).await?;
    let mut bc = state.get_bc().await;

    let interval_time = std::time::Duration::from_secs(5);
    let mut hb_interval = tokio::time::interval(interval_time);
    // a request handled inline (up to UNIX_TIMEOUT) delays ticks; firing them in a burst would
    // judge the heartbeat before the device could answer the ping and drop the connection
    hb_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut hb_received = true;

    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                if !hb_received {
                    error!("Closing connection due to no heartbeat ({:X})", esp_connect_info.id);
                    tracing::error!(file = format!("device_{:X}", esp_connect_info.id), "============= Closing connection (due to no heartbeat) =============");
                    break;
                }

                let msg = Message::Ping(vec![].into());
                socket.send(msg).await?;
                hb_received = false;
            }
            Ok(res) = bc.recv() => {
                match res {
                    crate::structs::BroadcastPacket::Build => {
                        let updates_allowed = state.inner.read().await.should_update;
                        if !updates_allowed {
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
                fkm_token: state.fkm_token,
                secure_rfid: state.secure_rfid,
                auto_setup: state.auto_setup,
                sound_enabled: state.sound_enabled,
            },
        }
    } else {
        TimerPacket {
            tag: None,
            data: TimerPacketInner::DeviceSettings {
                added: false,
                locales: state.locales.clone(),
                default_locale: state.default_locale.clone(),
                fkm_token: 0,
                secure_rfid: false,
                auto_setup: false,
                sound_enabled: state.sound_enabled,
            },
        }
    };

    drop(state);
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
        Message::Close(frame) => {
            if let Some(frame) = frame {
                info!(
                    "Closing connection ({}) Reason: {} ({:X})",
                    frame.code,
                    frame.reason.to_string(),
                    esp_connect_info.id
                );
                info!(
                    file = format!("device_{:X}", esp_connect_info.id),
                    "Closing connection ({}) Reason: {}",
                    frame.code,
                    frame.reason.to_string()
                );
            } else {
                info!("Closing connection");
            }
            return Ok(true);
        }
        Message::Pong(_) => {
            *hb_received = true;
        }
        Message::Text(payload) => {
            tracing::trace!("WS payload recv [{:X}]: {payload}", esp_connect_info.id);

            let response: TimerPacket = serde_json::from_str(&payload)?;
            let res = on_timer_response(socket, response, esp_connect_info, state).await;
            if let Err(e) = res {
                error!("on_timer_response error: {e:?}");
            }

            *hb_received = true;
        }
        Message::Binary(buf) => {
            let esp_id = esp_connect_info.id;
            if buf.first() == Some(&b'L') {
                match crate::log_packet::parse_log_packet(&buf) {
                    Ok(packet) => on_log_packet(packet, esp_id, state).await,
                    Err(e) => {
                        tracing::error!(
                            file = format!("device_{esp_id:X}"),
                            "Logs read error: {e}"
                        );
                    }
                }
            } else if buf.len() > 1 && buf[0] == b'C' {
                let error_log_buf = &buf[1..];
                if let Ok(parsed) = crate::error_log::parse_error_log_entries(error_log_buf) {
                    tracing::info!(
                        file = format!("device_{esp_id:X}"),
                        "DUMPED CRASH LOG: {parsed:#?}"
                    );
                } else {
                    tracing::info!(
                        file = format!("device_{esp_id:X}"),
                        "CRASH LOG INVALID FORMAT!"
                    );
                }
            }

            *hb_received = true;
        }

        _ => {}
    }

    Ok(false)
}

async fn on_log_packet(packet: LogPacket<'_>, esp_id: u32, state: &SharedAppState) {
    if packet.truncated {
        tracing::warn!(file = format!("device_{esp_id:X}"), "LOGS TRUNCATED!");
    }

    for line in packet.lines {
        const RESET: &str = "\u{001B}[0m";
        let color = match line.as_bytes().first() {
            Some(b'E') => "\u{001B}[31m",
            Some(b'W') => "\u{001B}[33m",
            Some(b'I') => "\u{001B}[32m",
            Some(b'D') => "\u{001B}[34m",
            Some(b'T') => "\u{001B}[35m",
            _ => "",
        };

        tracing::info!(file = format!("device_{esp_id:X}"), "{color}{line}{RESET}");
    }

    if packet.lines_malformed {
        tracing::error!(file = format!("device_{esp_id:X}"), "Logs read error!");
    }

    let inner_state = state.inner.read().await;
    if inner_state.devices_settings.contains_key(&esp_id) {
        drop(inner_state);
        _ = crate::socket::api::send_current_state(
            esp_id,
            packet.current_time,
            packet.inspection_time,
            packet.current_competitor,
            packet.current_group_id,
            packet.current_session_id,
        )
        .await;
    }
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
            is_competitor,
            attendance_device,
            sign_key,
        } => {
            let Some(device_settings) = state
                .inner
                .read()
                .await
                .devices_settings
                .get(&esp_id)
                .cloned()
            else {
                return Err(anyhow::anyhow!("Device not added"));
            };

            if let Some(dev_sign_key) = device_settings.sign_key
                && dev_sign_key != sign_key
            {
                return Err(anyhow::anyhow!("Wrong sign key!"));
            }

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

            let response = match crate::socket::api::get_competitor_info(
                card_id,
                esp_connect_info.id,
                is_competitor,
            )
            .await
            {
                Ok(info) => {
                    let registrant_display = match info.registrant_id {
                        Some(x) => format!(" ({x})"),
                        None => String::new(),
                    };

                    trace!("Card info: {} {:X} {:?}", card_id, esp_id, info);

                    TimerPacket {
                        tag: response.tag,
                        data: TimerPacketInner::CardInfoResponse {
                            card_id,
                            country_iso2: info.country_iso2.unwrap_or_default(),
                            display: format!("{}{}", info.name, registrant_display),
                            can_compete: info.can_compete,
                            possible_groups: info.possible_groups,
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
            sign_key,
        } => {
            let Some(device_settings) = state
                .inner
                .read()
                .await
                .devices_settings
                .get(&esp_id)
                .cloned()
            else {
                return Err(anyhow::anyhow!("Device not added"));
            };

            if let Some(dev_sign_key) = device_settings.sign_key
                && dev_sign_key != sign_key
            {
                return Err(anyhow::anyhow!("Wrong sign key!"));
            }

            trace!(
                "Solve: {solve_time} ({penalty}) {competitor_id} {esp_id:X} {timestamp} {session_id} {delegate} {group_id}"
            );
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
                Ok(unix_utils::response::UnixResponseData::EnterAttemptResp { message }) => {
                    if delegate {
                        return Ok(());
                    }

                    TimerPacket {
                        tag: response.tag,
                        data: TimerPacketInner::SolveConfirm {
                            session_id,
                            competitor_id,
                            message,
                        },
                    }
                }
                Ok(_) => {
                    if delegate {
                        return Ok(());
                    }

                    TimerPacket {
                        tag: response.tag,
                        data: TimerPacketInner::SolveConfirm {
                            session_id,
                            competitor_id,
                            message: "None".to_string(),
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
        TimerPacketInner::Battery { level, voltage: _ } => {
            let inner_state = state.inner.read().await;
            if inner_state.devices_settings.contains_key(&esp_id) {
                drop(inner_state);
                _ = crate::socket::api::send_battery_status(esp_id, level).await;
            }
        }
        TimerPacketInner::Add { firmware, sign_key } => {
            let inner_state = state.inner.read().await;
            if !inner_state.devices_settings.contains_key(&esp_id) {
                drop(inner_state);
                _ = crate::socket::api::add_device(
                    esp_id,
                    sign_key,
                    &esp_connect_info.hw,
                    &firmware,
                )
                .await;
                trace!("Add device: {:X}", esp_id);
            }
        }
        TimerPacketInner::TestAck(snapshot) => {
            let inner_state = state.inner.read().await;
            if inner_state.devices_settings.contains_key(&esp_id) {
                drop(inner_state);
                _ = crate::socket::api::send_test_ack(esp_id, snapshot).await;
            }
        }
        _ => {
            trace!("Not implemented timer response received: {:?}", response);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::DeviceSettings;
    use futures_util::{SinkExt, StreamExt};
    use std::{sync::Once, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream, UnixListener, UnixStream},
        time::timeout,
    };
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};
    use unix_utils::request::UnixRequestData;

    const DEVICE_ID: u32 = 1;
    const TIMEOUT: Duration = Duration::from_secs(2);

    type Device = WebSocketStream<MaybeTlsStream<TcpStream>>;

    fn provide_newer_firmware() {
        static SETUP: Once = Once::new();
        SETUP.call_once(|| {
            let dir = std::env::temp_dir().join(format!("fkm-fw-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            // the tail isn't utf8, so the version is taken from the file name
            std::fs::write(dir.join("v4_STATION_v3.5.0.bin"), [0xFF; 8192]).unwrap();
            // SAFETY: nothing else in the tests touches the environment
            unsafe { std::env::set_var("FIRMWARE_DIR", dir) };
        });
    }

    async fn state_with_device(should_update: bool) -> SharedAppState {
        let state = SharedAppState::new(false).await;
        let mut inner = state.inner.write().await;
        inner.should_update = should_update;
        inner
            .devices_settings
            .insert(DEVICE_ID, DeviceSettings { sign_key: None });
        drop(inner);
        state
    }

    async fn connect_device(state: &SharedAppState) -> Device {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::http::router(state.clone());
        tokio::spawn(async move { axum::serve(listener, app.into_make_service()).await });

        let url = format!("ws://{addr}/?id={DEVICE_ID}&ver=v3.4.4&hw=v4&firmware=STATION");
        connect_async(url).await.unwrap().0
    }

    async fn wait_for_frame(device: &mut Device, wanted: fn(&tungstenite::Message) -> bool) {
        let search = async {
            while let Some(frame) = device.next().await {
                if wanted(&frame.unwrap()) {
                    return;
                }
            }
            panic!("connection closed");
        };
        timeout(TIMEOUT, search).await.expect("frame not received");
    }

    async fn wait_for_tracking(state: &SharedAppState, connected: bool, within: Duration) {
        let settled = async {
            while state.connected_devices.contains(DEVICE_ID) != connected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        timeout(within, settled)
            .await
            .expect("tracking not updated");
    }

    fn is_start_update(frame: &tungstenite::Message) -> bool {
        frame.to_text().is_ok_and(|t| t.contains("start_update"))
    }

    // the first heartbeat goes out after the broadcast subscription
    async fn wait_until_listening(device: &mut Device) {
        wait_for_frame(device, tungstenite::Message::is_ping).await;
    }

    /// tokio's RwLock is fair: one reader that holds its guard for long is enough to park a
    /// writer, and with it every reader that comes after.
    async fn assert_readers_pass_a_writer(state: &SharedAppState) {
        let writer_state = state.clone();
        let writer = tokio::spawn(async move { drop(writer_state.inner.write().await) });
        tokio::task::yield_now().await;

        let reader = timeout(TIMEOUT, state.inner.read()).await;
        drop(reader.expect("reader blocked behind a writer"));
        timeout(TIMEOUT, writer)
            .await
            .expect("writer blocked")
            .unwrap();
    }

    async fn attach_backend(state: &SharedAppState) -> UnixStream {
        let path = std::env::temp_dir().join(format!("fkm-unix-{}", std::process::id()));
        let path = path.to_str().unwrap();
        let listener = UnixListener::bind(path).unwrap();
        crate::UNIX_SOCKET.init(path, state.clone()).await.unwrap();
        let (backend, _) = timeout(TIMEOUT, listener.accept()).await.unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
        backend
    }

    async fn read_request(backend: &mut UnixStream) -> String {
        let read = async {
            let mut frame = Vec::new();
            loop {
                match backend.read_u8().await.unwrap() {
                    0 => return String::from_utf8(frame).unwrap(),
                    byte => frame.push(byte),
                }
            }
        };
        timeout(TIMEOUT, read).await.expect("no backend request")
    }

    #[tokio::test]
    async fn update_on_connect_leaves_the_state_lock_free() {
        provide_newer_firmware();
        let state = state_with_device(true).await;

        let mut device = connect_device(&state).await;
        wait_for_frame(&mut device, is_start_update).await;

        assert_readers_pass_a_writer(&state).await;
    }

    #[tokio::test]
    async fn update_from_build_broadcast_leaves_the_state_lock_free() {
        provide_newer_firmware();
        let state = state_with_device(false).await;
        let mut device = connect_device(&state).await;
        wait_until_listening(&mut device).await;

        state.inner.write().await.should_update = true;
        state.build_broadcast().await.unwrap();
        wait_for_frame(&mut device, is_start_update).await;

        assert_readers_pass_a_writer(&state).await;
    }

    #[tokio::test]
    async fn clean_close_clears_the_device() {
        let state = state_with_device(false).await;
        let mut device = connect_device(&state).await;
        wait_for_tracking(&state, true, TIMEOUT).await;

        device.close(None).await.unwrap();
        wait_for_tracking(&state, false, TIMEOUT).await;
    }

    #[tokio::test]
    async fn broken_connection_clears_the_device() {
        let state = state_with_device(false).await;
        let device = connect_device(&state).await;
        wait_for_tracking(&state, true, TIMEOUT).await;

        // no close frame: the read fails and handle_client returns an error
        drop(device);
        wait_for_tracking(&state, false, TIMEOUT).await;
    }

    // a device that resets drops off without a word, and has to become configurable again
    #[tokio::test]
    async fn missed_heartbeat_clears_the_device() {
        let state = state_with_device(false).await;
        let _silent_device = connect_device(&state).await;
        wait_for_tracking(&state, true, TIMEOUT).await;

        wait_for_tracking(&state, false, Duration::from_secs(8)).await;
    }

    #[tokio::test]
    async fn overlapping_connections_keep_the_device_tracked() {
        let state = state_with_device(false).await;
        let mut old_device = connect_device(&state).await;
        wait_until_listening(&mut old_device).await;
        let mut new_device = connect_device(&state).await;
        wait_until_listening(&mut new_device).await;

        drop(old_device);
        // the old connection's task has to be gone, or this passes on its count alone
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(state.connected_devices.contains(DEVICE_ID));

        drop(new_device);
        wait_for_tracking(&state, false, TIMEOUT).await;
    }

    // one test because UNIX_SOCKET can only be initialised once
    #[tokio::test]
    async fn backend_traffic_does_not_wait_on_the_locks() {
        let state = state_with_device(false).await;
        let mut backend = attach_backend(&state).await;
        let mut device = connect_device(&state).await;
        wait_until_listening(&mut device).await;

        // tagged requests: the replies would have to come through the socket loop
        let battery = r#"{"data":{"battery":{"level":50.0}}}"#;
        device
            .send(tungstenite::Message::text(battery))
            .await
            .unwrap();
        assert!(read_request(&mut backend).await.contains("Battery"));
        assert_readers_pass_a_writer(&state).await;

        let log_state = state.clone();
        tokio::spawn(async move {
            on_log_packet(LogPacket::default(), DEVICE_ID, &log_state).await;
        });
        assert!(read_request(&mut backend).await.contains("CurrentTime"));
        assert_readers_pass_a_writer(&state).await;

        // ServerStatus parked on the state lock must not take outgoing requests down with it
        let reader = state.inner.read().await;
        let status = serde_json::json!({"type": "ServerStatus", "data": {
            "shouldUpdate": true, "devices": [], "translations": [], "defaultLocale": "en",
            "fkmToken": 1, "secureRfid": false, "autoSetup": false, "soundEnabled": false,
        }});
        backend
            .write_all(status.to_string().as_bytes())
            .await
            .unwrap();
        backend.write_u8(0).await.unwrap();
        let status_parked = async {
            while state.inner.try_read().is_ok() {
                tokio::task::yield_now().await;
            }
        };
        timeout(TIMEOUT, status_parked).await.expect("status lost");

        let request =
            crate::UNIX_SOCKET.send_async_request(UnixRequestData::RequestToConnectDevice {
                esp_id: DEVICE_ID,
                sign_key: 0,
                hw: "v4".to_string(),
                r#type: "STATION".to_string(),
            });
        timeout(TIMEOUT, request)
            .await
            .expect("request blocked behind ServerStatus")
            .unwrap();

        drop(reader);
        assert!(state.inner.read().await.should_update);
    }
}
