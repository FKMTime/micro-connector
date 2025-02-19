use serde::{Deserialize, Serialize};

pub mod request;
pub mod response;

#[derive(Debug)]
pub struct UnixError {
    pub message: String,
    pub should_reset_time: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TestPacketData {
    HardStateReset,
    ResetState,
    ScanCard(u64),
    ButtonPress { pin: u8, press_time: u64 },
    StackmatTime(u64),
    StackmatReset,
    Snapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapshotData {
    pub esp_id: u32,
    pub scene: u32,
    pub solve_session_id: String,
    pub solve_time: i64,
    pub last_solve_time: i64,
    pub penalty: i64,
    pub secondary_text: String,
    pub use_inspection: bool,
    pub inspection_started: u64,
    pub inspection_ended: u64,
    pub competitor_card_id: u64,
    pub judge_card_id: u64,
    pub competitor_display: String,
    pub time_confirmed: bool,
    pub error_msg: String,
    pub lcd_buffer: String,
    pub free_heap_size: u32,
}
