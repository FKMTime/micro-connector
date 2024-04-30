use super::structs::UnixError;
use crate::{socket::structs, structs::SnapshotData};
use anyhow::Result;

#[derive(Debug)]
pub struct CompetitorInfo {
    pub id: String,
    pub registrant_id: Option<i64>,
    pub name: String,
    pub wca_id: Option<String>,
    pub country_iso2: Option<String>,
    pub gender: String,
    pub can_compete: bool,
}

pub async fn get_competitor_info(card_id: u128) -> Result<CompetitorInfo, UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(structs::UnixRequestData::PersonInfo {
            card_id: card_id.to_string(),
        })
        .await?;

    if let structs::UnixResponseData::PersonInfoResp {
        id,
        registrant_id,
        name,
        wca_id,
        country_iso2,
        gender,
        can_compete,
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
        });
    }

    Err(UnixError {
        message: "Operation failed!".to_string(),
        should_reset_time: false,
    })
}

// For now, dont parse response (but its there)
pub async fn mark_attendance(esp_id: u32, card_id: u128) -> Result<(), UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(structs::UnixRequestData::CreateAttendance { card_id, esp_id })
        .await
        .map(|_| ());

    res
}

pub async fn send_snapshot_data(data: SnapshotData) -> Result<(), UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(structs::UnixRequestData::Snapshot(data))
        .await
        .map(|_| ());

    res
}

pub async fn send_solve_entry(
    time: u128,
    penalty: i64,
    solved_at: u128,
    esp_id: u32,
    judge_id: u128,
    competitor_id: u128,
    is_delegate: bool,
    session_id: &str,
    inspection_time: u128,
) -> Result<(), UnixError> {
    let time = time / 10; // Convert to centiseconds
    let solved_at = chrono::DateTime::from_timestamp_millis(solved_at as i64 * 1000)
        .ok_or_else(|| UnixError {
            message: format!("Error parsing timestamp"),
            should_reset_time: false,
        })?
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let data = structs::UnixRequestData::EnterAttempt {
        value: time,
        penalty,
        solved_at,
        esp_id,
        judge_id: judge_id.to_string(),
        competitor_id: competitor_id.to_string(),
        is_delegate,
        session_id: session_id.to_string(),
        inspection_time,
    };

    let res = crate::UNIX_SOCKET
        .send_tagged_request(data)
        .await
        .map(|_| ());

    res
}

pub async fn send_battery_status(esp_id: u32, battery: f64) -> Result<(), UnixError> {
    let battery: u8 = battery.round() as u8;

    let res = crate::UNIX_SOCKET
        .send_tagged_request(structs::UnixRequestData::UpdateBatteryPercentage {
            esp_id,
            battery_percentage: battery,
        })
        .await
        .map(|_| ());

    res
}

pub async fn add_device(esp_id: u32, firmware_type: &str) -> Result<(), UnixError> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(structs::UnixRequestData::RequestToConnectDevice {
            esp_id,
            r#type: firmware_type.to_string(),
        })
        .await
        .map(|_| ());

    res
}

pub async fn get_wifi_settings() -> Result<(String, String)> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(crate::socket::structs::UnixRequestData::WifiSettings)
        .await
        .map_err(|e| anyhow::anyhow!("Unix error: {e:?}"))?;

    if let crate::socket::structs::UnixResponseData::WifiSettingsResp {
        wifi_ssid,
        wifi_password,
    } = res
    {
        return Ok((wifi_ssid, wifi_password));
    }

    Err(anyhow::anyhow!("Cant get wifi settings!"))
}
