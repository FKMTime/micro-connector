use crate::{
    http::EspConnectInfo,
    structs::{SharedAppState, TimerPacket, TimerPacketInner},
};
use anyhow::Result;
use rand::RngExt;
use axum::extract::ws::{Message, WebSocket};
use tracing::{error, info, trace};

/// Constant-time compare for equal-length slices.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn parse_device_secret_hex(hex_str: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_str.trim())?;
    if bytes.len() != 32 {
        anyhow::bail!("device secret must be 32 bytes (64 hex chars), got {}", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// HMAC-SHA256(secret, "FKM-AUTH-V1" || esp_id_be || nonce_bytes) as 64-char hex lowercase.
fn compute_auth_mac(secret: &[u8; 32], esp_id: u32, nonce: &[u8]) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    let mut ctx = ring::hmac::Context::with_key(&key);
    ctx.update(b"FKM-AUTH-V1");
    ctx.update(&esp_id.to_be_bytes());
    ctx.update(nonce);
    let tag = ctx.sign();
    hex::encode(tag.as_ref())
}

pub async fn handle_client(
    mut socket: WebSocket,
    esp_connect_info: &EspConnectInfo,
    state: SharedAppState,
) -> Result<()> {
    tracing::info!(
        file = format!("device_{:X}", esp_connect_info.id),
        "============= Client connected! ============="
    );

    // Enrolled devices must complete connect-time HMAC before any privileged traffic.
    let mut session_authenticated = false;
    let enrolled_secret = {
        let state_inner = state.inner.read().await;
        state_inner
            .devices_settings
            .get(&esp_connect_info.id)
            .and_then(|s| s.sign_key.clone())
    };
    if let Some(secret_hex) = enrolled_secret {
        match run_connect_auth(&mut socket, esp_connect_info, &secret_hex).await {
            Ok(true) => {
                session_authenticated = true;
                info!(
                    "Device {:X} authenticated via connect HMAC",
                    esp_connect_info.id
                );
            }
            Ok(false) => {
                error!(
                    "Device {:X} failed connect HMAC — closing",
                    esp_connect_info.id
                );
                return Ok(());
            }
            Err(e) => {
                error!(
                    "Device {:X} auth handshake error: {e} — closing",
                    esp_connect_info.id
                );
                return Ok(());
            }
        }
    }

    {
        let state_inner = state.inner.read().await;
        // OTA only for authenticated enrolled sessions
        if session_authenticated
            && state_inner.should_update
            && let Some(firmware) = super::updater::should_update(&state, esp_connect_info).await?
        {
            tracing::info!(
                file = format!("device_{:X}", esp_connect_info.id),
                "Starting update."
            );
            super::updater::update_client(&mut socket, esp_connect_info, firmware).await?;
            return Ok(());
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
                        if !session_authenticated {
                            continue;
                        }
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
                        if !session_authenticated {
                            continue;
                        }
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
                let res = on_ws_msg(
                    &mut socket,
                    msg,
                    esp_connect_info,
                    &mut hb_received,
                    &state,
                    &mut session_authenticated,
                ).await;

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

/// Returns Ok(true) if auth succeeded, Ok(false) if rejected, Err on protocol/IO failure.
async fn run_connect_auth(
    socket: &mut WebSocket,
    esp_connect_info: &EspConnectInfo,
    secret_hex: &str,
) -> Result<bool> {
    let secret = parse_device_secret_hex(secret_hex)?;

    let mut nonce = [0u8; 32];
    rand::rng().fill(&mut nonce);
    let nonce_hex = hex::encode(nonce);

    let challenge = TimerPacket {
        tag: None,
        data: TimerPacketInner::AuthChallenge {
            nonce: nonce_hex.clone(),
        },
    };
    socket
        .send(Message::Text(serde_json::to_string(&challenge)?.into()))
        .await?;

    // Wait for AuthResponse (ignore pings); timeout 10s
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let fail = TimerPacket {
                tag: None,
                data: TimerPacketInner::AuthFail {
                    reason: "auth timeout".into(),
                },
            };
            let _ = socket
                .send(Message::Text(serde_json::to_string(&fail)?.into()))
                .await;
            return Ok(false);
        }

        let msg = tokio::time::timeout(remaining, socket.recv())
            .await
            .map_err(|_| anyhow::anyhow!("auth timeout"))?
            .ok_or_else(|| anyhow::anyhow!("socket closed during auth"))??;

        match msg {
            Message::Text(payload) => {
                let packet: TimerPacket = serde_json::from_str(&payload)?;
                match packet.data {
                    TimerPacketInner::AuthResponse { mac } => {
                        let expected = compute_auth_mac(&secret, esp_connect_info.id, &nonce);
                        let ok = ct_eq(expected.as_bytes(), mac.to_lowercase().as_bytes());
                        if ok {
                            let ok_pkt = TimerPacket {
                                tag: None,
                                data: TimerPacketInner::AuthOk,
                            };
                            socket
                                .send(Message::Text(serde_json::to_string(&ok_pkt)?.into()))
                                .await?;
                            return Ok(true);
                        } else {
                            let fail = TimerPacket {
                                tag: None,
                                data: TimerPacketInner::AuthFail {
                                    reason: "bad mac".into(),
                                },
                            };
                            socket
                                .send(Message::Text(serde_json::to_string(&fail)?.into()))
                                .await?;
                            return Ok(false);
                        }
                    }
                    other => {
                        trace!("Ignoring non-auth packet during handshake: {other:?}");
                    }
                }
            }
            Message::Ping(p) => {
                socket.send(Message::Pong(p)).await?;
            }
            Message::Close(_) => return Ok(false),
            _ => {}
        }
    }
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
    session_authenticated: &mut bool,
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
            let res = on_timer_response(
                socket,
                response,
                esp_connect_info,
                state,
                session_authenticated,
            )
            .await;
            if let Err(e) = res {
                error!("on_timer_response error: {e:?}");
            }

            *hb_received = true;
        }
        Message::Binary(buf) => {
            if !*session_authenticated {
                // Unauthenticated sessions may not upload logs/crash dumps
                *hb_received = true;
                return Ok(false);
            }

            let esp_id = esp_connect_info.id;
            if buf.len() > 10 && buf[0] == b'L' {
                //logs packet
                let current_time = if buf[2..10] != [0; 8] {
                    Some(u64::from_be_bytes(buf[2..10].try_into()?))
                } else {
                    None
                };

                if buf[1] == 0x01 {
                    tracing::warn!(file = format!("device_{esp_id:X}"), "LOGS TRUNCATED!");
                }

                let (lines, read_error) = parse_log_lines(&buf[10..])?;
                if read_error {
                    tracing::error!(file = format!("device_{esp_id:X}"), "Logs read error!");
                }

                for line in lines {
                    if !line.is_empty() {
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
                }

                if let Some(time) = current_time {
                    let inner_state = state.inner.read().await;
                    if inner_state.devices_settings.contains_key(&esp_id) {
                        _ = crate::socket::api::send_current_time(esp_id, time).await;
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

async fn on_timer_response(
    socket: &mut WebSocket,
    response: TimerPacket,
    esp_connect_info: &EspConnectInfo,
    state: &SharedAppState,
    session_authenticated: &mut bool,
) -> Result<()> {
    let esp_id = esp_connect_info.id;

    match response.data {
        // Auth packets only during handshake (handled elsewhere)
        TimerPacketInner::AuthChallenge { .. }
        | TimerPacketInner::AuthResponse { .. }
        | TimerPacketInner::AuthOk
        | TimerPacketInner::AuthFail { .. } => {
            trace!("Ignoring auth packet outside handshake");
        }

        TimerPacketInner::CardInfoRequest {
            card_id,
            is_competitor,
            attendance_device,
        } => {
            if !*session_authenticated {
                return Err(anyhow::anyhow!("Session not authenticated"));
            }
            if !state
                .inner
                .read()
                .await
                .devices_settings
                .contains_key(&esp_id)
            {
                return Err(anyhow::anyhow!("Device not added"));
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
        } => {
            if !*session_authenticated {
                return Err(anyhow::anyhow!("Session not authenticated"));
            }
            if !state
                .inner
                .read()
                .await
                .devices_settings
                .contains_key(&esp_id)
            {
                return Err(anyhow::anyhow!("Device not added"));
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
        TimerPacketInner::Logs { current_time, logs } => {
            if !*session_authenticated {
                return Ok(());
            }
            for log in logs.iter().rev() {
                for line in log.lines() {
                    if line.is_empty() {
                        continue;
                    }

                    tracing::info!(file = format!("device_{esp_id:X}"), "{line}");
                }
            }

            if let Some(time) = current_time {
                let inner_state = state.inner.read().await;
                if inner_state.devices_settings.contains_key(&esp_id) {
                    _ = crate::socket::api::send_current_time(esp_id, time).await;
                }
            }
        }
        TimerPacketInner::Battery { level, voltage: _ } => {
            if !*session_authenticated {
                return Ok(());
            }
            let inner_state = state.inner.read().await;
            if inner_state.devices_settings.contains_key(&esp_id) {
                _ = crate::socket::api::send_battery_status(esp_id, level).await;
            }
        }
        TimerPacketInner::Add {
            firmware,
            sign_key,
        } => {
            // Add is allowed without prior session auth (device is enrolling).
            // Validate hex secret format.
            if parse_device_secret_hex(&sign_key).is_err() {
                return Err(anyhow::anyhow!("Invalid sign_key format (need 64 hex chars)"));
            }

            let mut inner_state = state.inner.write().await;
            if !inner_state.devices_settings.contains_key(&esp_id) {
                // Cache secret locally so this session can act authenticated immediately;
                // backend ServerStatus will reconcile later.
                inner_state.devices_settings.insert(
                    esp_id,
                    crate::structs::DeviceSettings {
                        sign_key: Some(sign_key.clone()),
                    },
                );
                drop(inner_state);
                _ = crate::socket::api::add_device(
                    esp_id,
                    &sign_key,
                    &esp_connect_info.hw,
                    &firmware,
                )
                .await;
                *session_authenticated = true;
                trace!("Add device: {:X}", esp_id);
            }
        }
        TimerPacketInner::TestAck(snapshot) => {
            if !*session_authenticated {
                return Ok(());
            }
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

/// Parses device log lines from an `L` packet payload (without the 10-byte header).
/// Returns the parsed lines and a flag indicating malformed/truncated trailing data.
fn parse_log_lines(buf: &[u8]) -> Result<(Vec<&str>, bool)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    while offset + 2 <= buf.len() {
        let line_len = u16::from_be_bytes([buf[offset], buf[offset + 1]]) as usize;
        if offset + 2 + line_len > buf.len() {
            return Ok((lines, true));
        }

        lines.push(core::str::from_utf8(
            &buf[offset + 2..offset + 2 + line_len],
        )?);
        offset += 2 + line_len;
    }

    Ok((lines, offset != buf.len()))
}

#[cfg(test)]
mod tests {
    use super::parse_log_lines;

    fn packet(lines: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        for line in lines {
            buf.extend_from_slice(&(line.len() as u16).to_be_bytes());
            buf.extend_from_slice(line.as_bytes());
        }
        buf
    }

    #[test]
    fn parses_valid_lines() {
        let buf = packet(&["I hello", "E world", ""]);
        let (lines, truncated) = parse_log_lines(&buf).unwrap();
        assert_eq!(lines, ["I hello", "E world", ""]);
        assert!(!truncated);
    }

    #[test]
    fn empty_and_tiny_buffers_do_not_panic() {
        for len in 0..3 {
            let buf = vec![0xAA; len];
            let (_, truncated) = parse_log_lines(&buf).unwrap();
            assert_eq!(truncated, len != 0);
        }
    }

    #[test]
    fn declared_len_past_end() {
        let mut buf = packet(&["I ok"]);
        buf.extend_from_slice(&u16::MAX.to_be_bytes()); // claims 65535 bytes, none follow
        let (lines, truncated) = parse_log_lines(&buf).unwrap();
        assert_eq!(lines, ["I ok"]);
        assert!(truncated);
    }

    #[test]
    fn declared_len_exactly_at_end_off_by_two() {
        // regression: line_len + offset == buf.len() used to pass the check,
        // but the slice [offset+2 .. offset+2+line_len] went out of bounds
        let mut buf = Vec::new();
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(b"ab"); // only 2 of the 4 declared bytes present
        let (lines, truncated) = parse_log_lines(&buf).unwrap();
        assert!(lines.is_empty());
        assert!(truncated);
    }

    #[test]
    fn invalid_utf8_errors() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u16.to_be_bytes());
        buf.extend_from_slice(&[0xFF, 0xFF]);
        assert!(parse_log_lines(&buf).is_err());
    }

    #[test]
    fn random_buffers_do_not_panic() {
        // simple xorshift, no external deps
        let mut state = 0x9E3779B97F4A7C15u64;
        for _ in 0..1000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = (state % 64) as usize;
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                buf.push(state as u8);
            }
            _ = parse_log_lines(&buf); // must not panic
        }
    }
}
