use anyhow::Result;
use unix_utils::{
    request::UnixRequestData,
    response::{PossibleGroup, UnixResponseData},
    SnapshotData, UnixError,
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

pub async fn get_competitor_info(card_id: u64, esp_id: u32) -> Result<CompetitorInfo, UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::PersonInfo {
            card_id: card_id.to_string(),
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
            possible_groups: possible_groups.unwrap_or(
                [
                    PossibleGroup {
                        group_id: "333-r1".to_string(),
                        secondary_text: "3x3 R1".to_string(),
                        use_inspection: true,
                    },
                    PossibleGroup {
                        group_id: "222-r1".to_string(),
                        secondary_text: "2x2 R1".to_string(),
                        use_inspection: true,
                    },
                    PossibleGroup {
                        group_id: "other".to_string(),
                        secondary_text: "Other room".to_string(),
                        use_inspection: false,
                    },
                ]
                .to_vec(),
            ),
        });
    }

    Err(UnixError {
        message: "Operation failed!".to_string(),
        should_reset_time: false,
    })
}

// For now, dont parse response (but its there)
pub async fn mark_attendance(esp_id: u32, card_id: u64) -> Result<(), UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::CreateAttendance {
            card_id: card_id.to_string(),
            esp_id,
        })
        .await
        .map(|_| ());

    res
}

pub async fn send_snapshot_data(data: SnapshotData) -> Result<(), UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::Snapshot(data))
        .await
        .map(|_| ());

    res
}

pub async fn send_test_ack(esp_id: u32) -> Result<(), UnixError> {
    let data = UnixRequestData::TestAck { esp_id };
    crate::UNIX_SOCKET.send_async_request(data).await
}

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
    round_id: &str,
) -> Result<(), UnixError> {
    let solved_at = chrono::DateTime::from_timestamp_millis(solved_at as i64 * 1000)
        .ok_or_else(|| UnixError {
            message: format!("Error parsing timestamp"),
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
        round_id: round_id.to_string(),
    };

    let res = crate::UNIX_SOCKET
        .send_tagged_request(data)
        .await
        .map(|_| ());

    res
}

pub async fn send_battery_status(esp_id: u32, battery: Option<f64>) -> Result<(), UnixError> {
    if battery.is_none() {
        return Ok(());
    }

    let battery: u8 = battery.unwrap().round() as u8;
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::UpdateBatteryPercentage {
            esp_id,
            battery_percentage: battery,
        })
        .await
        .map(|_| ());

    res
}

pub async fn add_device(esp_id: u32, firmware_type: &str) -> Result<(), UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(UnixRequestData::RequestToConnectDevice {
            esp_id,
            r#type: firmware_type.to_string(),
        })
        .await
        .map(|_| ());

    res
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
