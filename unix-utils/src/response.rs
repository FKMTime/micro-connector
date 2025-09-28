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
pub struct AutoSetupSettings {
    pub ssid: String,
    pub psk: String,
    pub data: AutoSetupData,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AutoSetupData {
    pub mdns: bool,

    #[serde(rename = "wsUrl")]
    pub ws_url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixResponseData {
    AutoSetupSettingsResp(AutoSetupSettings),
    ServerStatus(CompetitionStatusResp),
    PersonInfoResp {
        id: String,
        registrant_id: Option<i64>,
        name: String,
        wca_id: Option<String>,
        country_iso2: Option<String>,
        gender: String,
        can_compete: bool,
        possible_groups: Vec<PossibleGroup>,
    },
    CustomMessage {
        esp_id: u32,
        line1: String,
        line2: String,
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
    UploadFirmware {
        file_name: String,
        file_data: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationLocale {
    pub locale: String,
    pub translations: Vec<TranslationRecord>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationRecord {
    pub key: String,
    pub translation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionStatusResp {
    pub should_update: bool,
    pub devices: Vec<CompetitionStatusDevice>,
    pub translations: Vec<TranslationLocale>,
    pub default_locale: String,
    pub fkm_token: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionStatusDevice {
    pub esp_id: u32,
    pub sign_key: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PossibleGroup {
    pub group_id: String,
    pub use_inspection: bool,
    pub secondary_text: String,
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

impl Default for CompetitionStatusResp {
    fn default() -> Self {
        Self {
            should_update: true,
            devices: Vec::new(),
            translations: Vec::new(),
            default_locale: "en".to_string(),
            fkm_token: 0,
        }
    }
}
