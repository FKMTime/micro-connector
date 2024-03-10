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
        competitor_id: u128,
        judge_id: u128,
        esp_id: u128,
        timestamp: u128,
        session_id: String, // UUID
        delegate: bool,
    },
    SolveConfirm {
        esp_id: u128,
        competitor_id: u128,
        session_id: String,
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
        can_compete: bool,
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

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStrategy {
    Disabled,
    Stable,
    Prerelease,
}

impl UpdateStrategy {
    pub fn get() -> Self {
        crate::UPDATE_STRATEGY
            .get()
            .expect("Should be set")
            .read()
            .map_or_else(|_| UpdateStrategy::Disabled, |x| x.clone())
    }

    pub fn should_update() -> bool {
        match Self::get() {
            UpdateStrategy::Disabled => false,
            UpdateStrategy::Stable => true,
            UpdateStrategy::Prerelease => true,
        }
    }

    pub fn set(strategy: UpdateStrategy) {
        crate::UPDATE_STRATEGY
            .get()
            .expect("Should be set")
            .write()
            .map(|mut x| *x = strategy)
            .expect("Should write");
    }
}
