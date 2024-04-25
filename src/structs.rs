use serde::Deserialize;
use std::collections::HashMap;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct LogData {
    pub millis: u128,
    pub msg: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TimerPacket {
    StartUpdate {
        esp_id: u32,
        version: String,
        build_time: u64, // NOT USED
        size: i64,
        firmware: String,
    },
    Solve {
        solve_time: u128,
        penalty: i64,
        competitor_id: u128,
        judge_id: u128,
        esp_id: u32,
        timestamp: u128,
        session_id: String, // UUID
        delegate: bool,
        inspection_time: u128,
    },
    SolveConfirm {
        esp_id: u32,
        competitor_id: u128,
        session_id: String,
    },
    DelegateResponse {
        esp_id: u32,
        should_scan_cards: bool,
        solve_time: u128,
        penalty: i64,
    },
    ApiError {
        esp_id: u32,
        error: String,
        should_reset_time: bool,
    },
    CardInfoRequest {
        card_id: u128,
        esp_id: u32,
        attendance_device: Option<bool>,
    },
    CardInfoResponse {
        card_id: u128,
        esp_id: u32,
        display: String,
        country_iso2: String,
        can_compete: bool,
    },
    AttendanceMarked {
        esp_id: u32,
    },
    DeviceSettings {
        esp_id: u32,
        use_inspection: bool,
        added: bool,
    },
    Logs {
        esp_id: u32,
        logs: Vec<LogData>,
    },
    Battery {
        esp_id: u32,
        level: f64,
        voltage: f64,
    },
    Add {
        esp_id: u32,
        firmware: String,
    },
}

#[derive(Debug, Clone)]
pub struct SharedAppState {
    inner: std::sync::Arc<tokio::sync::RwLock<AppState>>,
    build_bc: tokio::sync::broadcast::Sender<()>,
    resp_bc: tokio::sync::broadcast::Sender<(u32, TimerPacket)>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub should_update: bool,
    pub devices_settings: HashMap<u32, CompetitionDeviceSettings>,
}

impl SharedAppState {
    pub async fn new() -> Self {
        let (resp_bc, _) = tokio::sync::broadcast::channel(1024);
        let (build_bc, _) = tokio::sync::broadcast::channel(10);

        Self {
            inner: std::sync::Arc::new(tokio::sync::RwLock::new(AppState {
                should_update: false,
                devices_settings: HashMap::new(),
            })),
            build_bc,
            resp_bc,
        }
    }

    pub async fn build_broadcast(&self) -> anyhow::Result<()> {
        self.build_bc.send(())?;
        Ok(())
    }

    pub async fn send_timer_packet(&self, esp_id: u32, packet: TimerPacket) -> anyhow::Result<()> {
        self.resp_bc.send((esp_id, packet))?;
        Ok(())
    }

    pub async fn get_build_bc(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.build_bc.subscribe()
    }

    pub async fn get_timer_packet_bc(
        &self,
    ) -> tokio::sync::broadcast::Receiver<(u32, TimerPacket)> {
        self.resp_bc.subscribe()
    }
}

#[derive(Debug, Clone)]
pub struct CompetitionDeviceSettings {
    pub use_inspection: bool,
}

// API RESPONSE
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionStatusResp {
    pub should_update: bool,
    pub devices: Vec<u32>,
    pub rooms: Vec<Room>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub use_inspection: bool,
    pub devices: Vec<u32>,
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WifiSettings {
    pub wifi_ssid: String,
    pub wifi_password: String,
}
