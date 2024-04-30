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
        use_inspection: Option<bool>,
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

    // packet for end to end testing
    TestPacket(TestPacketData),
    Snapshot(SnapshotData),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TestPacketData {
    Start,
    End,
    ResetState,
    ScanCard(u64),
    ButtonPress { pins: Vec<u8>, press_time: u64 },
    SolveTime(u64),
    Snapshot,
}

#[derive(Debug, Clone)]
pub enum BroadcastPacket {
    Build,
    Resp((u32, TimerPacket)),
    UpdateDeviceSettings,
}

#[derive(Debug, Clone)]
pub struct SharedAppState {
    pub inner: std::sync::Arc<tokio::sync::RwLock<AppState>>,
    pub dev_mode: bool,
    bc: tokio::sync::broadcast::Sender<BroadcastPacket>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub should_update: bool,
    pub devices_settings: HashMap<u32, CompetitionDeviceSettings>,
}

impl SharedAppState {
    pub async fn new(dev_mode: bool) -> Self {
        let (bc, _) = tokio::sync::broadcast::channel(1024);

        Self {
            dev_mode,
            inner: std::sync::Arc::new(tokio::sync::RwLock::new(AppState {
                should_update: false,
                devices_settings: HashMap::new(),
            })),
            bc,
        }
    }

    pub async fn build_broadcast(&self) -> anyhow::Result<()> {
        self.bc.send(BroadcastPacket::Build)?;
        Ok(())
    }

    pub async fn device_settings_broadcast(&self) -> anyhow::Result<()> {
        self.bc.send(BroadcastPacket::UpdateDeviceSettings)?;
        Ok(())
    }

    pub async fn send_timer_packet(&self, esp_id: u32, packet: TimerPacket) -> anyhow::Result<()> {
        self.bc.send(BroadcastPacket::Resp((esp_id, packet)))?;
        Ok(())
    }

    pub async fn get_bc(&self) -> tokio::sync::broadcast::Receiver<BroadcastPacket> {
        self.bc.subscribe()
    }
}

#[derive(Debug, Clone)]
pub struct CompetitionDeviceSettings {
    pub use_inspection: Option<bool>,
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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct SnapshotData {
    pub esp_id: u32,
    pub scene: u32,
    pub solve_session_id: String,
    pub solve_time: i64,
    pub last_solve_time: i64,
    pub penalty: i64,
    pub use_inspection: bool,
    pub inspection_started: u64,
    pub inspection_ended: u64,
    pub competitor_card_id: u64,
    pub judge_card_id: u64,
    pub competitor_display: String,
    pub time_confirmed: bool,
    pub error_msg: String,
}
