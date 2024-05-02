use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc::UnboundedSender, RwLock};
use unix_utils::request::UnixRequestData;

pub type SharedSenders = Arc<RwLock<HashMap<u32, UnboundedSender<UnixRequestData>>>>;
pub struct State {
    pub devices: Vec<u32>,
    pub senders: SharedSenders,
    pub tests: Arc<RwLock<TestsRoot>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorInfo {
    pub registrant_id: i64,
    pub name: String,
    pub wca_id: String,
    pub can_compete: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestsRoot {
    pub dump_state_after_test: bool,
    pub cards: HashMap<u64, CompetitorInfo>,
    pub buttons: HashMap<String, Vec<u8>>,
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
    Snapshot,
    ResetState,
    SolveTime(u64),
    SolveTimeRng,
    Button {
        name: String,
        time: u64,
    },
    DelegateResolve {
        should_scan_cards: bool,
        penalty: i64,
        value: u64,
    },

    // verifiers
    VerifySolveTime {
        time: Option<u64>,
        penalty: i64,
    },
    VerifyDelegateSent,
    // TODO: VerifySnapshotState (with options like in delegate)
}
