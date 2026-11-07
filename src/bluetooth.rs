use crate::adapter::BleAdapter;
use crate::structs::SharedAppState;
use anyhow::Result;
use btleplug::api::{Central, Manager as _, Peripheral, ScanFilter, WriteType};
use btleplug::platform::Manager;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{Instant, timeout};

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

                tokio::time::sleep(Duration::from_secs(5)).await;
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

            tokio::time::sleep(Duration::from_secs(5)).await;
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
        tokio::time::sleep(Duration::from_secs(5)).await;
        if !state.inner.read().await.auto_setup {
            continue;
        }

        let filter = ScanFilter { services: vec![] };
        adapter.start_scan(filter).await?;

        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut found = Vec::new();
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
            // BlueZ also lists cached devices; one it didn't hear in this scan should have no rssi
            tracing::trace!("\"{local_name}\" rssi: {:?}", properties.rssi);
            found.push((device, local_name));
        }

        if !found.is_empty()
            && let Some(set_wifi_data) = load_set_wifi_data().await
        {
            for (device, local_name) in found {
                if let Err(e) = setup_bt_device(&device, &local_name, &set_wifi_data).await {
                    tracing::error!("Failed to setup BT device \"{local_name}\": {:?}", e);
                }
            }
        }

        adapter.stop_scan().await?;
    }
}

async fn adapter_bluetooth_task(state: &SharedAppState, adapter: &mut BleAdapter) -> Result<()> {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if !state.inner.read().await.auto_setup {
            continue;
        }

        let Ok(scan_devices) = adapter.scan(Duration::from_secs(5)).await else {
            continue;
        };

        let found: Vec<_> = scan_devices
            .into_iter()
            .filter(|device| should_setup_device(state, &device.local_name))
            .collect();
        if found.is_empty() {
            continue;
        }

        let Some(set_wifi_data) = load_set_wifi_data().await else {
            continue;
        };

        for device in found {
            tracing::info!("Found FKM device with name: \"{}\"!", device.local_name);

            let res = adapter
                .write_to_device(device.device_id, SET_WIFI_UUID, &set_wifi_data)
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

/// Fetched once per pass, before connecting to anything: without settings there is nothing to
/// write, and connecting anyway would only hold devices that accept a single connection.
async fn load_set_wifi_data() -> Option<Vec<u8>> {
    tracing::trace!("Getting wifi settings");
    let auto_setup_settings = match crate::socket::api::get_auto_setup_settings().await {
        Ok(ass) => ass,
        Err(api_err) => match std::env::var("AUTOSETUP_SETTINGS") {
            Ok(ass) => ass,
            Err(_) => {
                tracing::warn!(
                    "No wifi settings (API: {api_err:?}, AUTOSETUP_SETTINGS unset), skipping this pass"
                );
                return None;
            }
        },
    };

    set_wifi_data(&auto_setup_settings)
}

fn set_wifi_data(auto_setup_settings: &str) -> Option<Vec<u8>> {
    match serde_json::from_str::<AutoSetupSettings>(auto_setup_settings) {
        Ok(ass) if ass.ssid.is_empty() => {
            tracing::trace!("Wifi ssid empty, skipping this pass");
            None
        }
        Ok(_) => {
            tracing::trace!("Got wifi settings");
            Some(format!("{auto_setup_settings}\0").into_bytes())
        }
        Err(e) => {
            tracing::error!("Wifi auto setup settings parse error: {e:?}, skipping this pass");
            None
        }
    }
}

// Each step is bounded, so one unreachable device can't stall the queue behind it. The work
// takes at most 25s and the disconnect 5s, so a device costs at most 30s.
const IS_CONNECTED_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(8);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The BLE operations of one setup, apart so the sequence can be tested without hardware.
trait SetupDevice {
    async fn is_connected(&self) -> Result<bool>;
    async fn connect(&self) -> Result<()>;
    async fn discover_services(&self) -> Result<()>;
    async fn write_set_wifi(&self, data: &[u8]) -> Result<()>;
    async fn disconnect(&self) -> Result<()>;
}

impl SetupDevice for btleplug::platform::Peripheral {
    async fn is_connected(&self) -> Result<bool> {
        Ok(Peripheral::is_connected(self).await?)
    }

    async fn connect(&self) -> Result<()> {
        Ok(Peripheral::connect(self).await?)
    }

    async fn discover_services(&self) -> Result<()> {
        Ok(Peripheral::discover_services(self).await?)
    }

    async fn write_set_wifi(&self, data: &[u8]) -> Result<()> {
        let characteristics = self.characteristics();
        let set_wifi = characteristics
            .iter()
            .find(|c| c.uuid == SET_WIFI_UUID)
            .ok_or_else(|| anyhow::anyhow!("Couldn't find SET_WIFI characteristic!"))?;

        Ok(Peripheral::write(self, set_wifi, data, WriteType::WithoutResponse).await?)
    }

    async fn disconnect(&self) -> Result<()> {
        Ok(Peripheral::disconnect(self).await?)
    }
}

// The disconnect runs after every attempt, whatever its outcome: a device in setup mode accepts
// one connection, so one left open hides it until BlueZ or the connector restarts. It also
// cancels a `connect()` that timed out while still pending.
async fn setup_bt_device(
    device: &impl SetupDevice,
    local_name: &str,
    set_wifi_data: &[u8],
) -> Result<()> {
    let started = Instant::now();
    let res = write_set_wifi(device, local_name, set_wifi_data).await;

    let disconnect_started = Instant::now();
    match timeout(DISCONNECT_TIMEOUT, device.disconnect()).await {
        Ok(Ok(())) => tracing::trace!(
            "\"{local_name}\" disconnect: ok in {:?}",
            disconnect_started.elapsed()
        ),
        Ok(Err(e)) => tracing::warn!("\"{local_name}\" disconnect failed: {e:?}"),
        Err(_) => {
            tracing::warn!("\"{local_name}\" disconnect timed out after {DISCONNECT_TIMEOUT:?}")
        }
    }

    tracing::trace!("\"{local_name}\" setup took {:?}", started.elapsed());
    res
}

async fn write_set_wifi(
    device: &impl SetupDevice,
    local_name: &str,
    set_wifi_data: &[u8],
) -> Result<()> {
    let connected = step(
        local_name,
        "is_connected",
        IS_CONNECTED_TIMEOUT,
        device.is_connected(),
    )
    .await?;
    if !connected {
        step(local_name, "connect", CONNECT_TIMEOUT, device.connect()).await?;
    }

    step(
        local_name,
        "discover",
        DISCOVER_TIMEOUT,
        device.discover_services(),
    )
    .await?;

    // WithoutResponse: an ok here means the write was queued, not that the device got it
    tracing::trace!(
        "\"{local_name}\" writing {} bytes WithoutResponse",
        set_wifi_data.len()
    );
    step(
        local_name,
        "write",
        WRITE_TIMEOUT,
        device.write_set_wifi(set_wifi_data),
    )
    .await?;

    tracing::info!("Wrote wifi settings to device");
    Ok(())
}

async fn step<T>(
    local_name: &str,
    name: &str,
    limit: Duration,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let started = Instant::now();
    let res = timeout(limit, work).await;
    let elapsed = started.elapsed();

    match &res {
        Ok(Ok(_)) => tracing::trace!("\"{local_name}\" {name}: ok in {elapsed:?}"),
        Ok(Err(e)) => tracing::trace!("\"{local_name}\" {name}: failed in {elapsed:?}: {e:?}"),
        Err(_) => tracing::trace!("\"{local_name}\" {name}: timed out after {elapsed:?}"),
    }

    res.map_err(|_| anyhow::anyhow!("{name} timed out after {limit:?}"))?
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

    #[derive(Clone, Copy)]
    enum Step {
        Ok,
        Fail,
        Hang,
        Take(Duration),
    }

    impl Step {
        async fn run(self) -> Result<()> {
            match self {
                Step::Ok => Ok(()),
                Step::Fail => Err(anyhow::anyhow!("step failed")),
                Step::Hang => std::future::pending().await,
                Step::Take(duration) => {
                    tokio::time::sleep(duration).await;
                    Ok(())
                }
            }
        }
    }

    struct FakeDevice {
        connected: bool,
        connect: Step,
        discover: Step,
        write: Step,
        disconnect: Step,
        calls: std::sync::Mutex<Vec<&'static str>>,
    }

    impl FakeDevice {
        fn new() -> Self {
            Self {
                connected: false,
                connect: Step::Ok,
                discover: Step::Ok,
                write: Step::Ok,
                disconnect: Step::Ok,
                calls: Default::default(),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.lock().unwrap().push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SetupDevice for FakeDevice {
        async fn is_connected(&self) -> Result<bool> {
            self.record("is_connected");
            Ok(self.connected)
        }

        async fn connect(&self) -> Result<()> {
            self.record("connect");
            self.connect.run().await
        }

        async fn discover_services(&self) -> Result<()> {
            self.record("discover");
            self.discover.run().await
        }

        async fn write_set_wifi(&self, _data: &[u8]) -> Result<()> {
            self.record("write");
            self.write.run().await
        }

        async fn disconnect(&self) -> Result<()> {
            self.record("disconnect");
            self.disconnect.run().await
        }
    }

    async fn setup(device: &FakeDevice) -> Result<()> {
        setup_bt_device(device, DEVICE_NAME, b"{}\0").await
    }

    #[tokio::test(start_paused = true)]
    async fn successful_setup_disconnects() {
        let device = FakeDevice::new();

        assert!(setup(&device).await.is_ok());
        assert_eq!(
            device.calls(),
            ["is_connected", "connect", "discover", "write", "disconnect"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn device_already_connected_is_not_connected_again_but_is_disconnected() {
        let device = FakeDevice {
            connected: true,
            ..FakeDevice::new()
        };

        assert!(setup(&device).await.is_ok());
        assert_eq!(
            device.calls(),
            ["is_connected", "discover", "write", "disconnect"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failed_step_still_disconnects() {
        for (device, last_step) in [
            (
                FakeDevice {
                    connect: Step::Fail,
                    ..FakeDevice::new()
                },
                "connect",
            ),
            (
                FakeDevice {
                    discover: Step::Fail,
                    ..FakeDevice::new()
                },
                "discover",
            ),
            // also how a missing SET_WIFI characteristic fails
            (
                FakeDevice {
                    write: Step::Fail,
                    ..FakeDevice::new()
                },
                "write",
            ),
        ] {
            let res = setup(&device).await;

            assert_eq!(res.unwrap_err().to_string(), "step failed", "{last_step}");
            let calls = device.calls();
            assert_eq!(calls[calls.len() - 2..], [last_step, "disconnect"]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_step_times_out_and_still_disconnects() {
        for (device, last_step, limit) in [
            (
                FakeDevice {
                    connect: Step::Hang,
                    ..FakeDevice::new()
                },
                "connect",
                CONNECT_TIMEOUT,
            ),
            (
                FakeDevice {
                    discover: Step::Hang,
                    ..FakeDevice::new()
                },
                "discover",
                DISCOVER_TIMEOUT,
            ),
            (
                FakeDevice {
                    write: Step::Hang,
                    ..FakeDevice::new()
                },
                "write",
                WRITE_TIMEOUT,
            ),
        ] {
            let start = Instant::now();
            let res = setup(&device).await;

            assert_eq!(
                res.unwrap_err().to_string(),
                format!("{last_step} timed out after {limit:?}")
            );
            assert_eq!(start.elapsed(), limit);
            let calls = device.calls();
            assert_eq!(calls[calls.len() - 2..], [last_step, "disconnect"]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_disconnect_is_bounded_and_keeps_setup_result() {
        let device = FakeDevice {
            disconnect: Step::Hang,
            ..FakeDevice::new()
        };
        let start = Instant::now();

        assert!(setup(&device).await.is_ok());
        assert_eq!(start.elapsed(), DISCONNECT_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn slowest_setup_stays_within_30_seconds() {
        let just_under = |limit: Duration| Step::Take(limit - Duration::from_millis(1));
        let device = FakeDevice {
            connect: just_under(CONNECT_TIMEOUT),
            discover: just_under(DISCOVER_TIMEOUT),
            write: just_under(WRITE_TIMEOUT),
            disconnect: Step::Hang,
            ..FakeDevice::new()
        };
        let start = Instant::now();

        assert!(setup(&device).await.is_ok());
        assert!(start.elapsed() < Duration::from_secs(30));
        assert_eq!(
            IS_CONNECTED_TIMEOUT
                + CONNECT_TIMEOUT
                + DISCOVER_TIMEOUT
                + WRITE_TIMEOUT
                + DISCONNECT_TIMEOUT,
            Duration::from_secs(30)
        );
    }

    #[test]
    fn settings_are_written_nul_terminated() {
        let settings = r#"{"ssid":"example-net","psk":"example-psk","data":null}"#;

        assert_eq!(
            set_wifi_data(settings),
            Some(format!("{settings}\0").into_bytes())
        );
    }

    #[test]
    fn settings_without_an_ssid_are_not_written() {
        assert_eq!(set_wifi_data(r#"{"ssid":"","psk":"example-psk"}"#), None);
    }

    #[test]
    fn settings_that_do_not_parse_are_not_written() {
        assert_eq!(set_wifi_data(""), None);
        assert_eq!(set_wifi_data(r#"{"psk":"example-psk"}"#), None);
    }
}
