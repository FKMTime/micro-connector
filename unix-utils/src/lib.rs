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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapshotData {
    pub scene: usize,
    pub inspection_time: Option<u64>,
    pub solve_time: Option<u64>,
    pub penalty: Option<i8>,
    pub time_confirmed: bool,
    pub possible_groups: usize,
    pub group_selected_idx: usize,
    pub current_competitor: Option<u64>,
    pub current_judge: Option<u64>,
}
