use anyhow::Result;
use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::Manager;
use futures::StreamExt;

const FKM_UUID: uuid::Uuid = uuid::Uuid::from_u128(0x3ee59312_20bc_4c38_9e23_e785b6c385b1);
const SET_WIFI_UUID: uuid::Uuid = uuid::Uuid::from_u128(0xe2ed1fc5_0d2e_4c2d_a0a7_31e38431cc0c);

pub async fn start_bluetooth_task() -> Result<()> {
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
    let (api_client, api_url) = crate::api::ApiClient::get_api_client()?;

    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No adapters found"))?;

    // idk why but this filter doesn't seem to work
    let filter = ScanFilter {
        services: vec![FKM_UUID],
    };

    let mut events = adapter.events().await?;
    adapter.start_scan(filter).await?;

    while let Some(event) = events.next().await {
        match event {
            CentralEvent::DeviceDiscovered(id) => {
                let device = adapter.peripheral(&id).await?;
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

                device.connect().await?;
                device.discover_services().await?;

                let characteristics = device.characteristics();
                let set_wifi = characteristics
                    .iter()
                    .find(|c| c.uuid == SET_WIFI_UUID)
                    .ok_or_else(|| anyhow::anyhow!("Couldn't find SET_WIFI characteristic!"))?;

                // get wifi settings from API or env
                let (ssid, psk) = if let Ok((ssid, psk)) =
                    crate::api::get_wifi_settings(&api_client, &api_url).await
                {
                    (ssid, psk)
                } else {
                    let ssid = std::env::var("WIFI_SSID")?;
                    let psk = std::env::var("WIFI_PSK")?;
                    (ssid, psk)
                };
                println!("ssid: {}, psk: {}", ssid, psk);

                let set_wifi_data = format!("{ssid}|{psk}");
                let set_wifi_data = set_wifi_data.as_bytes();

                device
                    .write(
                        set_wifi,
                        set_wifi_data,
                        btleplug::api::WriteType::WithoutResponse,
                    )
                    .await?;
            }
            _ => {}
        }
    }

    Ok(())
}
