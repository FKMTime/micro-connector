use crate::SnapshotData;
use serde::{Deserialize, Serialize};

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
        esp_id: u32,
    },
    AutoSetupSettings,
    CreateAttendance {
        card_id: String,
        esp_id: u32,
    },
    EnterAttempt {
        value: u64,
        value_ms: u64,
        penalty: i64,
        solved_at: String,
        esp_id: u32,
        judge_id: String,
        competitor_id: String,
        is_delegate: bool,
        session_id: String,
        inspection_time: i64,
        group_id: String,
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
    TestAck {
        esp_id: u32,
        snapshot: SnapshotData,
    },
}
