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
        esp_id: u128,
        version: String,
        build_time: u128, // NOT USED
        size: i64,
    },
    Solve {
        solve_time: u128,
        offset: i64,
        solver_id: u128,
        judge_id: u128,
        esp_id: u128,
        timestamp: u128,
        session_id: i64,
        delegate: bool,
    },
    SolveConfirm {
        esp_id: u128,
        solver_id: u128,
        session_id: i64,
    },
    ApiError {
        esp_id: u128,
        error: String,
        should_reset_time: bool,
    },
    CardInfoRequest {
        card_id: u128,
        esp_id: u128,
    },
    CardInfoResponse {
        card_id: u128,
        esp_id: u128,
        display: String,
        country_iso2: String,
    },
    Logs {
        esp_id: u128,
        logs: Vec<LogData>,
    },
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubReleaseItem {
    pub name: String,
    pub tag: String,
    pub url: String,
}
