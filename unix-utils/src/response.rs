use crate::TestPacketData;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UnixResponse {
    pub error: Option<bool>,
    pub tag: Option<u32>,

    #[serde(flatten)]
    pub data: Option<UnixResponseData>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixResponseData {
    WifiSettingsResp2(String),
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
        possible_rounds: Option<Vec<PossibleRound>>, // TODO: remove option,
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
        data: TestPacketData,
    },
    Empty,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionStatusResp {
    pub should_update: bool,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub devices: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PossibleRound {
    pub id: String,
    pub name: String,
    pub use_inspection: bool,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiSettings {
    pub wifi_ssid: String,
    pub wifi_password: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IncidentAttempt {
    pub session_id: String,
    pub penalty: Option<i64>,
    pub value: Option<u64>,
}
