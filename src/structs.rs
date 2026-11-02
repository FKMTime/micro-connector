use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use unix_utils::{
    SnapshotData, TestPacketData,
    response::{PossibleGroup, TranslationLocale},
};

use crate::updater::Firmware;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TimerPacket {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<u64>,
    pub data: TimerPacketInner,
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
        group_id: String,
        sign_key: u32,
    },
    SolveConfirm {
        competitor_id: u64,
        session_id: String,
        message: String,
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
    CustomMessage {
        line1: String,
        line2: String,
    },
    CardInfoRequest {
        card_id: u64,
        is_competitor: bool,

        #[serde(skip_serializing_if = "Option::is_none")]
        attendance_device: Option<bool>,

        sign_key: u32,
    },
    CardInfoResponse {
        card_id: u64,
        display: String,
        country_iso2: String,
        can_compete: bool,
        possible_groups: Vec<PossibleGroup>,
    },
    AttendanceMarked,
    DeviceSettings {
        added: bool,
        locales: Vec<TranslationLocale>,
        default_locale: String,
        fkm_token: i32,
        secure_rfid: bool,
        auto_setup: bool,
        sound_enabled: bool,
    },
    Battery {
        level: Option<f64>,
        voltage: Option<f64>,
    },
    Add {
        firmware: String,
        sign_key: u32,
    },
    EpochTime {
        current_epoch: u64,
    },
    SetDeviceSettings {
        volume: Option<u8>,
    },
    DumpCrashLog,

    // packet for end to end testing
    TestPacket(TestPacketData),
    TestAck(SnapshotData),
}

#[derive(Debug, Clone)]
pub enum BroadcastPacket {
    Build,
    Resp((u32, TimerPacket)),
    UpdateDeviceSettings,
    ForceUpdate((String, Firmware)),
}

#[derive(Debug, Clone)]
pub struct SharedAppState {
    pub inner: std::sync::Arc<tokio::sync::RwLock<AppState>>,
    pub dev_mode: bool,
    pub connected_devices: ConnectedDevices,
    bc: tokio::sync::broadcast::Sender<BroadcastPacket>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub should_update: bool,
    pub devices_settings: HashMap<u32, DeviceSettings>,
    pub locales: Vec<TranslationLocale>,
    pub default_locale: String,
    pub fkm_token: i32,
    pub secure_rfid: bool,
    pub auto_setup: bool,
    pub sound_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSettings {
    pub sign_key: Option<u32>,
}

/// Devices with a live websocket connection, counted per id: a device can reconnect before its
/// old connection is dropped, and closing the old one must not hide the new one.
///
/// Kept out of `AppState` because [`ConnectionGuard`] updates it from `Drop`, which can't await.
/// The lock is only ever held inside the methods below, never across an await.
#[derive(Debug, Clone, Default)]
pub struct ConnectedDevices(Arc<Mutex<HashMap<u32, usize>>>);

/// Marks a device as connected until dropped, so every way out of a connection task clears it.
#[must_use = "the device stops counting as connected when this is dropped"]
#[derive(Debug)]
pub struct ConnectionGuard {
    devices: ConnectedDevices,
    esp_id: u32,
}

impl ConnectedDevices {
    fn counts(&self) -> MutexGuard<'_, HashMap<u32, usize>> {
        // no panic can leave the map half updated, and this also runs in Drop during an unwind
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn track(&self, esp_id: u32) -> ConnectionGuard {
        *self.counts().entry(esp_id).or_insert(0) += 1;

        ConnectionGuard {
            devices: self.clone(),
            esp_id,
        }
    }

    pub fn contains(&self, esp_id: u32) -> bool {
        self.counts().contains_key(&esp_id)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut counts = self.devices.counts();
        if let Some(count) = counts.get_mut(&self.esp_id) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.esp_id);
            }
        }
    }
}

impl SharedAppState {
    pub async fn new(dev_mode: bool) -> Self {
        let (bc, _) = tokio::sync::broadcast::channel(1024);

        Self {
            dev_mode,
            connected_devices: ConnectedDevices::default(),
            inner: std::sync::Arc::new(tokio::sync::RwLock::new(AppState {
                should_update: false,
                devices_settings: HashMap::new(),
                locales: Vec::new(),
                default_locale: "en".to_string(),
                fkm_token: 0,
                secure_rfid: false,
                auto_setup: false,
                sound_enabled: true,
            })),
            bc,
        }
    }

    pub async fn build_broadcast(&self) -> anyhow::Result<()> {
        self.bc.send(BroadcastPacket::Build)?;
        Ok(())
    }

    pub async fn force_update(&self, hw: String, firmware: Firmware) -> anyhow::Result<()> {
        self.bc.send(BroadcastPacket::ForceUpdate((hw, firmware)))?;
        Ok(())
    }

    pub async fn device_settings_broadcast(&self) -> anyhow::Result<()> {
        self.bc.send(BroadcastPacket::UpdateDeviceSettings)?;
        self.bc.send(BroadcastPacket::Build)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_ID: u32 = 1;
    const OTHER_DEVICE_ID: u32 = 2;

    #[test]
    fn device_counts_as_connected_while_the_guard_lives() {
        let devices = ConnectedDevices::default();
        assert!(!devices.contains(DEVICE_ID));

        let guard = devices.track(DEVICE_ID);
        assert!(devices.contains(DEVICE_ID));
        assert!(!devices.contains(OTHER_DEVICE_ID));

        drop(guard);
        assert!(!devices.contains(DEVICE_ID));
    }

    #[test]
    fn closing_an_old_connection_keeps_the_newer_one() {
        let devices = ConnectedDevices::default();
        let old = devices.track(DEVICE_ID);
        let new = devices.track(DEVICE_ID);

        drop(old);
        assert!(devices.contains(DEVICE_ID));

        drop(new);
        assert!(!devices.contains(DEVICE_ID));
    }

    #[test]
    fn connections_can_close_in_any_order() {
        let devices = ConnectedDevices::default();
        let old = devices.track(DEVICE_ID);
        let new = devices.track(DEVICE_ID);

        drop(new);
        assert!(devices.contains(DEVICE_ID));

        drop(old);
        assert!(!devices.contains(DEVICE_ID));
    }

    #[tokio::test]
    async fn panic_in_a_connection_task_clears_the_device() {
        let devices = ConnectedDevices::default();

        let task_devices = devices.clone();
        let task = tokio::spawn(async move {
            let _connected = task_devices.track(DEVICE_ID);
            panic!("connection task failed");
        });

        assert!(task.await.unwrap_err().is_panic());
        assert!(!devices.contains(DEVICE_ID));
    }

    #[tokio::test]
    async fn aborted_connection_task_clears_the_device() {
        let devices = ConnectedDevices::default();

        let task_devices = devices.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _connected = task_devices.track(DEVICE_ID);
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_rx.await.unwrap();
        assert!(devices.contains(DEVICE_ID));

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!devices.contains(DEVICE_ID));
    }
}
