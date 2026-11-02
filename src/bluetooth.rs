use crate::adapter::BleAdapter;
use crate::structs::SharedAppState;
use anyhow::Result;
use btleplug::api::{Central, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::Manager;
use serde::Deserialize;

//const FKM_UUID: uuid::Uuid = uuid::uuid!("f254a578-ef88-4372-b5f5-5ecf87e65884");
const SET_WIFI_UUID: uuid::Uuid = uuid::uuid!("bcd7e573-b0b2-4775-83c0-acbf3aaf210c");
const FKM_NAME_PREFIX: &str = "FKM-";

/// Firmware advertises `FKM-{:X}` of the same id it connects to the websocket with.
fn parse_device_id(local_name: &str) -> Option<u32> {
    let hex_id = local_name.strip_prefix(FKM_NAME_PREFIX)?;

    // from_str_radix alone would also take a leading `+`
    if !hex_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }

    u32::from_str_radix(hex_id, 16).ok()
}

/// Whether a scanned device is worth a write. A device online over websocket stopped listening
/// on bluetooth, so writing to it only burns a full timeout. This is checked against live
/// connections only, with no memory of past writes: a reset device, or one given a wrong wifi
/// password, has to be retried on the next pass.
fn should_setup_device(state: &SharedAppState, local_name: &str) -> bool {
    if !local_name.starts_with(FKM_NAME_PREFIX) {
        return false;
    }

    let connected =
        parse_device_id(local_name).is_some_and(|id| state.connected_devices.contains(id));
    if connected {
        tracing::trace!("Skipping \"{local_name}\": already connected");
    }

    !connected
}

pub async fn start_bluetooth_task(state: SharedAppState) -> Result<()> {
    let adapter_api = crate::adapter::adapter_api_url();
    let client = reqwest::Client::new();
    if let Ok(res) = client.get(&adapter_api).send().await
        && res.status().is_success()
    {
        tokio::task::spawn(async move {
            let mut ble_adapter = BleAdapter::new(client, adapter_api);
            loop {
                if let Err(e) = adapter_bluetooth_task(&state, &mut ble_adapter).await {
                    tracing::error!("Adapter Bluetooth task failed: {:?}", e);
                }

                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        return Ok(());
    }

    let manager = Manager::new().await?;
    if manager.adapters().await.is_err() {
        tracing::error!("No bluetooth adapter found!");
        return Ok(());
    }

    tokio::task::spawn(async move {
        loop {
            if let Err(e) = bluetooth_task(&state).await {
                tracing::error!("Bluetooth task failed: {:?}", e);
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    Ok(())
}

async fn bluetooth_task(state: &SharedAppState) -> Result<()> {
    let manager = Manager::new().await?;
    manager.adapters().await?;

    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No adapters found"))?;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if !state.inner.read().await.auto_setup {
            continue;
        }

        let filter = ScanFilter { services: vec![] };
        adapter.start_scan(filter).await?;

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        for device in adapter.peripherals().await? {
            // one peripheral that went away shouldn't cost the rest of the pass
            let properties = match device.properties().await {
                Ok(Some(properties)) => properties,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("Failed to read BT device properties: {:?}", e);
                    continue;
                }
            };

            //let is_fkm = properties.services.contains(&FKM_UUID);
            let local_name = properties.local_name.unwrap_or_default();
            if !should_setup_device(state, &local_name) {
                continue;
            }

            tracing::info!("Found FKM device with name: \"{local_name}\"!");

            let res = setup_bt_device(device).await;
            if let Err(e) = res {
                tracing::error!("Failed to setup BT device: {:?}", e);
            }
        }

        adapter.stop_scan().await?;
    }
}

async fn adapter_bluetooth_task(state: &SharedAppState, adapter: &mut BleAdapter) -> Result<()> {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if !state.inner.read().await.auto_setup {
            continue;
        }

        let Ok(scan_devices) = adapter.scan(std::time::Duration::from_secs(5)).await else {
            continue;
        };

        for device in scan_devices {
            if !should_setup_device(state, &device.local_name) {
                continue;
            }

            tracing::info!("Found FKM device with name: \"{}\"!", device.local_name);

            tracing::trace!("Getting wifi settings");
            let auto_setup_settings =
                if let Ok(ass) = crate::socket::api::get_auto_setup_settings().await {
                    ass
                } else {
                    std::env::var("AUTOSETUP_SETTINGS")?
                };

            let res = serde_json::from_str::<AutoSetupSettings>(&auto_setup_settings);
            match res {
                Ok(ass) => {
                    if ass.ssid.is_empty() {
                        tracing::trace!("Wifi ssid null... Skipping!");
                        return Ok(());
                    }
                }
                Err(e) => {
                    tracing::error!("Wifi auto setup settings parse error: {e:?}");
                    return Ok(());
                }
            }

            let set_wifi_data = format!("{auto_setup_settings}\0");
            let set_wifi_data = set_wifi_data.as_bytes();
            tracing::trace!("Got wifi settings");

            let res = adapter
                .write_to_device(device.device_id, SET_WIFI_UUID, set_wifi_data)
                .await;
            if let Err(e) = res {
                tracing::error!("Failed to setup BT device: {:?}", e);
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AutoSetupSettings {
    pub ssid: String,
    pub psk: String,
    pub data: Option<ConnSettings>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct ConnSettings {
    pub mdns: bool,
    pub ws_url: Option<String>,
}

async fn setup_bt_device(device: btleplug::platform::Peripheral) -> Result<()> {
    if !device.is_connected().await? {
        tracing::trace!("Connecting to device");
        device.connect().await?;
    }

    tracing::trace!("Connected to device");
    device.discover_services().await?;
    tracing::trace!("Discovered services");

    tracing::trace!("Getting characteristics");
    let characteristics = device.characteristics();
    let set_wifi = characteristics
        .iter()
        .find(|c| c.uuid == SET_WIFI_UUID)
        .ok_or_else(|| anyhow::anyhow!("Couldn't find SET_WIFI characteristic!"))?;
    tracing::trace!("Got characteristics");

    // get wifi settings from API or env
    tracing::trace!("Getting wifi settings");
    let auto_setup_settings = if let Ok(ass) = crate::socket::api::get_auto_setup_settings().await {
        ass
    } else {
        std::env::var("AUTOSETUP_SETTINGS")?
    };

    let res = serde_json::from_str::<AutoSetupSettings>(&auto_setup_settings);
    match res {
        Ok(ass) => {
            if ass.ssid.is_empty() {
                tracing::trace!("Wifi ssid null... Skipping!");
                return Ok(());
            }
        }
        Err(e) => {
            tracing::error!("Wifi auto setup settings parse error: {e:?}");
            return Ok(());
        }
    }

    let set_wifi_data = format!("{auto_setup_settings}\0");
    let set_wifi_data = set_wifi_data.as_bytes();
    tracing::trace!("Got wifi settings");

    _ = device
        .write(
            set_wifi,
            set_wifi_data,
            btleplug::api::WriteType::WithoutResponse,
        )
        .await;

    tracing::info!("Wrote wifi settings to device");
    _ = device.disconnect().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE_ID: u32 = 0x1A2B3C;
    const DEVICE_NAME: &str = "FKM-1A2B3C";

    #[test]
    fn parses_the_name_the_firmware_builds() {
        assert_eq!(
            parse_device_id(&format!("FKM-{DEVICE_ID:X}")),
            Some(DEVICE_ID)
        );
        // no zero padding in the firmware format
        assert_eq!(parse_device_id("FKM-1"), Some(1));
        // the largest id the firmware can produce
        assert_eq!(parse_device_id("FKM-7FFFFFFF"), Some(0x7FFF_FFFF));
    }

    #[test]
    fn hex_digits_match_in_either_case() {
        assert_eq!(parse_device_id("FKM-1a2b3c"), Some(DEVICE_ID));
    }

    #[test]
    fn rejects_names_that_are_not_a_device_id() {
        for name in [
            "",
            "FKM-",
            "FKM",
            "fkm-1A2B3C",
            "FKM-XYZ",
            "FKM-+1A2B3C",
            "FKM--1",
            "FKM- 1A2B3C",
            "FKM-1A2B3C ",
            "FKM-100000000",
            "Other-1A2B3C",
        ] {
            assert_eq!(parse_device_id(name), None, "{name:?}");
        }
    }

    #[tokio::test]
    async fn connected_device_is_skipped() {
        let state = SharedAppState::new(false).await;
        let _connected = state.connected_devices.track(DEVICE_ID);

        assert!(!should_setup_device(&state, DEVICE_NAME));
    }

    #[tokio::test]
    async fn device_without_a_connection_is_tried() {
        let state = SharedAppState::new(false).await;
        let _connected = state.connected_devices.track(DEVICE_ID + 1);

        assert!(should_setup_device(&state, DEVICE_NAME));
    }

    #[tokio::test]
    async fn fkm_name_that_does_not_parse_is_tried() {
        let state = SharedAppState::new(false).await;

        assert!(should_setup_device(&state, "FKM-"));
        assert!(should_setup_device(&state, "FKM-XYZ"));
        assert!(should_setup_device(&state, "FKM-100000000"));
    }

    #[tokio::test]
    async fn other_devices_are_not_set_up() {
        let state = SharedAppState::new(false).await;

        assert!(!should_setup_device(&state, ""));
        assert!(!should_setup_device(&state, "Headphones"));
    }

    #[tokio::test]
    async fn device_is_tried_again_on_every_pass() {
        let state = SharedAppState::new(false).await;

        // a write that never led to a connection, like a wrong wifi password
        assert!(should_setup_device(&state, DEVICE_NAME));
        assert!(should_setup_device(&state, DEVICE_NAME));
    }

    #[tokio::test]
    async fn device_that_went_offline_is_tried_again() {
        let state = SharedAppState::new(false).await;

        let connected = state.connected_devices.track(DEVICE_ID);
        assert!(!should_setup_device(&state, DEVICE_NAME));

        drop(connected);
        assert!(should_setup_device(&state, DEVICE_NAME));
    }
}
