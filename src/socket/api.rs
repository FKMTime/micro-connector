use crate::log_packet::LogPacket;
use anyhow::Result;
use unix_utils::{
    SnapshotData, UnixError,
    request::UnixRequestData,
    response::{PossibleGroup, UnixResponseData},
};

#[derive(Debug)]
#[allow(dead_code)]
pub struct CompetitorInfo {
    pub id: String,
    pub registrant_id: Option<i64>,
    pub name: String,
    pub wca_id: Option<String>,
    pub country_iso2: Option<String>,
    pub gender: String,
    pub can_compete: bool,
    pub possible_groups: Vec<PossibleGroup>,
}

pub async fn get_competitor_info(
    card_id: u64,
    esp_id: u32,
    is_competitor: bool,
) -> Result<CompetitorInfo, UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::PersonInfo {
            card_id: card_id.to_string(),
            is_competitor,
            esp_id,
        })
        .await?;

    if let UnixResponseData::PersonInfoResp {
        id,
        registrant_id,
        name,
        wca_id,
        country_iso2,
        gender,
        can_compete,
        possible_groups,
    } = res
    {
        return Ok(CompetitorInfo {
            id,
            registrant_id,
            name,
            wca_id,
            country_iso2,
            gender,
            can_compete,
            possible_groups,
        });
    }

    Err(UnixError {
        message: "Operation failed!".to_string(),
        should_reset_time: false,
    })
}

// For now, dont parse response (but its there)
pub async fn mark_attendance(esp_id: u32, card_id: u64) -> Result<(), UnixError> {
    crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::CreateAttendance {
            card_id: card_id.to_string(),
            esp_id,
        })
        .await
        .map(|_| ())
}

pub async fn send_test_ack(esp_id: u32, snapshot: SnapshotData) -> Result<(), UnixError> {
    let data = UnixRequestData::TestAck { esp_id, snapshot };
    crate::UNIX_SOCKET.send_async_request(data).await
}

#[allow(clippy::too_many_arguments)]
pub async fn send_solve_entry(
    time: u64,
    penalty: i64,
    solved_at: u64,
    esp_id: u32,
    judge_id: u64,
    competitor_id: u64,
    is_delegate: bool,
    session_id: &str,
    inspection_time: i64,
    group_id: &str,
) -> Result<UnixResponseData, UnixError> {
    let solved_at = chrono::DateTime::from_timestamp_millis(solved_at as i64 * 1000)
        .ok_or_else(|| UnixError {
            message: "Error parsing timestamp".to_string(),
            should_reset_time: false,
        })?
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let data = UnixRequestData::EnterAttempt {
        value: time / 10,
        value_ms: time,
        penalty,
        solved_at,
        esp_id,
        judge_id: judge_id.to_string(),
        competitor_id: competitor_id.to_string(),
        is_delegate,
        session_id: session_id.to_string(),
        inspection_time,
        group_id: group_id.to_string(),
    };

    crate::UNIX_SOCKET.send_tagged_request(data).await
}

pub async fn send_battery_status(esp_id: u32, battery: Option<f64>) -> Result<(), UnixError> {
    let Some(battery) = battery else {
        return Ok(());
    };

    let battery: u8 = battery.round() as u8;

    crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::UpdateBatteryPercentage {
            esp_id,
            battery_percentage: battery,
        })
        .await
        .map(|_| ())
}

pub async fn send_device_connected(esp_id: u32, firmware_version: &str) -> Result<(), UnixError> {
    crate::UNIX_SOCKET
        .send_async_request(UnixRequestData::DeviceConnected {
            esp_id,
            firmware_version: firmware_version.to_string(),
        })
        .await
}

pub async fn send_current_state(esp_id: u32, packet: LogPacket<'_>) -> Result<(), UnixError> {
    crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::CurrentTimeInfo {
            esp_id,
            time: packet.current_time,
            inspection: packet.inspection_time,
            competitor: packet.current_competitor,
            group_id: packet.current_group_id,
            session_id: packet.current_session_id,
            timer_running: packet.timer_running,
            inspection_running: packet.inspection_running,
        })
        .await
        .map(|_| ())
}

pub async fn add_device(
    esp_id: u32,
    sign_key: u32,
    hw: &str,
    firmware_type: &str,
) -> Result<(), UnixError> {
    crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::RequestToConnectDevice {
            esp_id,
            sign_key,
            hw: hw.to_string(),
            r#type: firmware_type.to_string(),
        })
        .await
        .map(|_| ())
}

pub async fn get_auto_setup_settings() -> Result<String> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::AutoSetupSettings)
        .await
        .map_err(|e| anyhow::anyhow!("Unix error: {e:?}"))?;

    if let UnixResponseData::AutoSetupSettingsResp(resp) = res {
        return Ok(serde_json::to_string(&resp)?);
    }

    Err(anyhow::anyhow!("Cant get auto setup settings!"))
}

#[cfg(test)]
mod tests {
    use unix_utils::request::{UnixRequest, UnixRequestData};

    fn current_time_info(
        timer_running: Option<bool>,
        inspection_running: Option<bool>,
    ) -> serde_json::Value {
        let req = UnixRequest {
            tag: Some(7),
            data: UnixRequestData::CurrentTimeInfo {
                esp_id: 0xA1B2,
                time: Some(1_000),
                inspection: None,
                competitor: None,
                group_id: None,
                session_id: None,
                timer_running,
                inspection_running,
            },
        };

        serde_json::to_value(&req).unwrap()
    }

    #[test]
    fn current_time_info_sends_live_state() {
        let json = current_time_info(Some(true), Some(false));

        assert_eq!(json["type"], "CurrentTimeInfo");
        assert_eq!(json["data"]["timerRunning"], true);
        assert_eq!(json["data"]["inspectionRunning"], false);
    }

    /// The backend tells old firmware apart by the explicit null.
    #[test]
    fn current_time_info_sends_null_without_live_state() {
        let json = current_time_info(None, None);
        let data = json["data"].as_object().unwrap();

        assert_eq!(data.get("timerRunning"), Some(&serde_json::Value::Null));
        assert_eq!(
            data.get("inspectionRunning"),
            Some(&serde_json::Value::Null)
        );
    }
}
