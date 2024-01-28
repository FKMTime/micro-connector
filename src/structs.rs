#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct LogData {
    pub millis: u128,
    pub msg: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TimerResponse {
    StartUpdate {
        esp_id: u128,
        version: String,
        size: i64,
    },
    Solve {
        solve_time: u128,
        offset: i64,
        card_id: u128,
        esp_id: u128,
        timestamp: u128,
        session_id: i64,
        delegate: bool,
    },
    SolveConfirm {
        esp_id: u128,
        card_id: u128,
        session_id: i64,
    },
    CardInfoRequest {
        card_id: u128,
        esp_id: u128,
    },
    CardInfoResponse {
        card_id: u128,
        esp_id: u128,
        name: String,
        is_judge: bool,
    },
    Logs {
        esp_id: u128,
        logs: Vec<LogData>,
    },
}
