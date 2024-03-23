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
        build_time: u128, // NOT USED
        size: i64,
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
    },
    CardInfoResponse {
        card_id: u128,
        esp_id: u32,
        display: String,
        country_iso2: String,
        can_compete: bool,
    },
    DeviceSettings {
        esp_id: u32,
        use_inspection: bool,
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
    pub release_channel: ReleaseChannel,
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
    pub release_channel: ReleaseChannel,
    pub devices: Vec<u32>,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub enum ReleaseChannel {
    #[serde(rename = "STABLE")]
    Stable,

    #[serde(rename = "PRE_RELEASE")]
    Prerelease,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub use_inspection: bool,
    pub devices: Vec<u32>,
}
