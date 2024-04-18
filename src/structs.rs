use std::collections::HashMap;

use serde::Deserialize;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct LogData {
    pub millis: u128,
    pub msg: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TimerResponse {
    StartUpdate {
        esp_id: u32,
        version: String,
        build_time: u64, // NOT USED
        size: i64,
        firmware: String,
    },
    Solve {
        solve_time: u128,
        penalty: i64,
        competitor_id: u128,
        judge_id: u128,
        esp_id: u32,
        timestamp: u128,
        session_id: String, // UUID
        delegate: bool,
        inspection_time: u128,
    },
    SolveConfirm {
        esp_id: u32,
        competitor_id: u128,
        session_id: String,
    },
    ApiError {
        esp_id: u32,
        error: String,
        should_reset_time: bool,
    },
    CardInfoRequest {
        card_id: u128,
        esp_id: u32,
        attendance_device: Option<bool>,
    },
    CardInfoResponse {
        card_id: u128,
        esp_id: u32,
        display: String,
        country_iso2: String,
        can_compete: bool,
    },
    AttendanceMarked {
        esp_id: u32,
    },
    DeviceSettings {
        esp_id: u32,
        use_inspection: bool,
        added: bool,
    },
    Logs {
        esp_id: u32,
        logs: Vec<LogData>,
    },
    Battery {
        esp_id: u32,
        level: f64,
        voltage: f64,
    },
    Add {
        esp_id: u32,
        firmware: String,
    },
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubReleaseItem {
    pub name: String,
    pub tag: String,
    pub url: String,
}

pub type SharedCompetitionStatus = std::sync::Arc<tokio::sync::RwLock<CompetitionStatus>>;

#[derive(Debug, Clone)]
pub struct CompetitionStatus {
    pub should_update: bool,
    pub devices_settings: HashMap<u32, CompetitionDeviceSettings>,
}

#[derive(Debug, Clone)]
pub struct CompetitionDeviceSettings {
    pub use_inspection: bool,
}

// API RESPONSE
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionStatusResp {
    pub should_update: bool,
    pub devices: Vec<u32>,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub use_inspection: bool,
    pub devices: Vec<u32>,
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiSettings {
    pub wifi_ssid: String,
    pub wifi_password: String,
}
