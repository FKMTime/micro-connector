use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use unix_utils::{SnapshotData, TestPacketData};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TimerPacket {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<u64>,
    pub data: TimerPacketInner,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogData {
    pub millis: u64,
    pub msg: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TimerPacketInner {
    StartUpdate {
        version: String,
        build_time: u64, // NOT USED
        size: u32,
        crc: u32,
        firmware: String,
    },
    Solve {
        solve_time: u64,
        penalty: i64,
        competitor_id: u64,
        judge_id: u64,
        timestamp: u64,
        session_id: String, // UUID
        delegate: bool,
        inspection_time: i64,
    },
    SolveConfirm {
        competitor_id: u64,
        session_id: String,
    },
    DelegateResponse {
        should_scan_cards: bool,

        #[serde(skip_serializing_if = "Option::is_none")]
        solve_time: Option<u64>,

        #[serde(skip_serializing_if = "Option::is_none")]
        penalty: Option<i64>,
    },
    ApiError {
        error: String,
        should_reset_time: bool,
    },
    CardInfoRequest {
        card_id: u64,

        #[serde(skip_serializing_if = "Option::is_none")]
        attendance_device: Option<bool>,
    },
    CardInfoResponse {
        card_id: u64,
        display: String,
        country_iso2: String,
        can_compete: bool,
    },
    AttendanceMarked,
    DeviceSettings {
        use_inspection: bool,
        secondary_text: String,
        added: bool,
    },
    Logs {
        logs: Vec<LogData>,
    },
    Battery {
        level: Option<f64>,
        voltage: Option<f64>,
    },
    Add {
        firmware: String,
    },
    EpochTime {
        current_epoch: u64,
    },

    // packet for end to end testing
    TestPacket(TestPacketData),
    Snapshot(SnapshotData),
    TestAck,
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

#[derive(Debug, Clone, PartialEq)]
pub struct CompetitionDeviceSettings {
    pub use_inspection: bool,
    pub secondary_text: String,
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
