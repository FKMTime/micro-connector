use serde::{Deserialize, Serialize};
use crate::structs::{CompetitionStatusResp, SnapshotData, TestPacketData};

#[derive(Debug)]
pub struct UnixError {
    pub message: String,
    pub should_reset_time: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct UnixResponse {
    pub error: Option<bool>,
    pub tag: Option<u32>,

    #[serde(flatten)]
    pub data: Option<UnixResponseData>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixResponseData {
    WifiSettingsResp {
        wifi_ssid: String,
        wifi_password: String,
    },
    ServerStatus(CompetitionStatusResp),
    PersonInfoResp {
        id: String,
        registrant_id: Option<i64>,
        name: String,
        wca_id: Option<String>,
        country_iso2: Option<String>,
        gender: String,
        can_compete: bool,
    },
    Error {
        message: String,
        should_reset_time: bool,
    },
    Success {
        message: String,
    },
    IncidentResolved {
        esp_id: u32,
        should_scan_cards: bool,
        attempt: IncidentAttempt,
    },
    TestPacket {
        esp_id: u32,
        data: TestPacketData
    },
    Empty,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixRequest {
    pub tag: Option<u32>,

    #[serde(flatten)]
    pub data: UnixRequestData,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixRequestData {
    PersonInfo {
        card_id: String,
    },
    WifiSettings,
    CreateAttendance {
        card_id: u128,
        esp_id: u32,
    },
    EnterAttempt {
        value: u128,
        penalty: i64,
        solved_at: String,
        esp_id: u32,
        judge_id: String,
        competitor_id: String,
        is_delegate: bool,
        session_id: String,
        inspection_time: u128,
    },
    UpdateBatteryPercentage {
        esp_id: u32,
        battery_percentage: u8,
    },
    RequestToConnectDevice {
        esp_id: u32,

        #[serde(rename = "type")]
        r#type: String,
    },
    Snapshot(SnapshotData),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IncidentAttempt {
    pub session_id: String,
    pub penalty: i64,
    pub value: u64,
    pub inspection_time: u64,
}
