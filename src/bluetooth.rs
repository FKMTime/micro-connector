use anyhow::Result;
use btleplug::api::{Central, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::Manager;

const FKM_UUID: uuid::Uuid = uuid::Uuid::from_u128(0x3ee59312_20bc_4c38_9e23_e785b6c385b1);
const SET_WIFI_UUID: uuid::Uuid = uuid::Uuid::from_u128(0xe2ed1fc5_0d2e_4c2d_a0a7_31e38431cc0c);

pub async fn start_bluetooth_task() -> Result<()> {
    let manager = Manager::new().await?;
    if manager.adapters().await.is_err() {
        tracing::error!("No bluetooth adapter found!");
        return Ok(());
    }

    tokio::task::spawn(async move {
        loop {
            if let Err(e) = bluetooth_task().await {
                tracing::error!("Bluetooth task failed: {:?}", e);
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    Ok(())
}

async fn bluetooth_task() -> Result<()> {
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

        let filter = ScanFilter {
            services: vec![FKM_UUID],
        };
        adapter.start_scan(filter).await?;

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        for device in adapter.peripherals().await? {
            let properties = device
                .properties()
                .await?
                .ok_or_else(|| anyhow::anyhow!("No device properties found!"))?;

            let is_fkm = properties.services.contains(&FKM_UUID);
            if !is_fkm {
                continue;
            }

            tracing::info!(
                "Found FKM device with name: \"{}\"!",
                properties.local_name.unwrap_or("none".to_string())
            );

            let res = setup_bt_device(device).await;
            if let Err(e) = res {
                tracing::error!("Failed to setup BT device: {:?}", e);
            }
        }

        adapter.stop_scan().await?;
    }
}

async fn setup_bt_device(device: btleplug::platform::Peripheral) -> Result<()> {
    tracing::trace!("Connecting to device");
    device.connect().await?;
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
