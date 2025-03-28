use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use unix_utils::{
    request::UnixRequestData,
    response::{CompetitionStatusResp, PossibleGroup, UnixResponse},
    SnapshotData,
};

#[derive(Clone)]
pub struct HilState {
    pub devices: Vec<HilDevice>,
    pub tests: TestsRoot,
    pub should_send_status: bool,
    pub status: CompetitionStatusResp,

    pub completed_count: usize,
    pub packet_queue: Vec<UnixResponse>,

    pub get_ms: fn() -> u64,
    pub log_fn: fn(&str, String),

    pub error_log: Vec<HilErrorLoc>,
}

#[derive(Clone, Debug)]
pub struct HilErrorLoc {
    pub test: usize,
    pub step: usize,
    pub error: HilError,
}

#[derive(Clone)]
pub struct HilDevice {
    pub id: u32,
    pub last_snapshot: Option<SnapshotData>,
    pub back_packet: Option<UnixRequestData>,
    pub next_step_time: u64,

    pub current_test: Option<usize>,
    pub current_step: usize,
    pub wait_for_ack: bool,

    pub last_test: usize,
    pub last_solve_time: u64,

    pub completed_count: usize,
    pub errored: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CardInfo {
    pub registrant_id: i64,
    pub name: String,
    pub wca_id: String,
    pub can_compete: bool,
    pub groups: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestsRoot {
    pub dump_state_after_test: bool,
    pub groups: Vec<PossibleGroup>,
    pub cards: HashMap<u64, CardInfo>,
    pub buttons: HashMap<String, u8>,
    pub tests: Vec<TestData>,
}

const DEFAULT_SLEEP_BETWEEN: u64 = 500; //500ms
fn default_sleep_between() -> u64 {
    DEFAULT_SLEEP_BETWEEN
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestData {
    pub name: String,

    #[serde(default = "default_sleep_between")]
    pub sleep_between: u64,

    pub steps: Vec<TestStep>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum TestStep {
    Sleep(u64),
    ScanCard(u64),
    ResetState,

    /// Simulate timer time (random)
    SolveTime,
    Button {
        name: String,
        time: u64,
        ack: Option<bool>,
    },
    DelegateResolve {
        should_scan_cards: bool,
        penalty: Option<i64>,
        value: Option<u64>,
    },

    VerifySend {
        /// If none, this wont be checked,
        /// if -1 this will check against random generated timer time,
        /// if any other value this will check exact value
        time: Option<i64>,

        /// If none, this wont be checked,
        /// If any value this will check exact value
        penalty: Option<i64>,

        /// If true this will check if delegate request was sent
        delegate: bool,
    },

    /// List of dsl "queries"
    VerifySnapshot(Vec<String>),
}

#[derive(Clone, Debug)]
pub enum HilError {
    TimeoutAck,
    WrongButtonName,
    BackpacketTimeout,
    BackpacketWrong,
    SnapshotDsl(String),
    StepNotMatched,
    ValueNotExpected,
}
