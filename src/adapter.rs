use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use uuid::Uuid;

pub fn adapter_api_url() -> String {
    std::env::var("ADAPTER_API").unwrap_or_else(|_| {
        if is_running_in_docker() {
            "http://host.docker.internal:3127"
        } else {
            "http://localhost:3127"
        }
        .to_string()
    })
}

pub fn is_running_in_docker() -> bool {
    if Path::new("/.dockerenv").exists() {
        return true;
    }

    if let Ok(contents) = std::fs::read_to_string("/proc/self/cgroup")
        && contents.contains("docker")
    {
        return true;
    }

    false
}

#[derive(Debug, Serialize)]
struct RegisterMdnsApi {
    pub all_interfaces: bool,
    pub properties: Vec<(String, String)>,
    pub service_type: String,
    pub instance_name: String,
    pub ip: Option<String>,
    pub port: u16,
    pub host_name: String,
}

pub async fn register_mdns(client: reqwest::Client, api_url: String, port: u16) -> Result<()> {
    tracing::info!("Using MDNS Adapter api!");
    let mut data = RegisterMdnsApi {
        all_interfaces: true,
        properties: Vec::new(),
        service_type: crate::mdns::SERVICE_TYPE.to_string(),
        instance_name: crate::mdns::INSTANCE_NAME.to_string(),
        ip: None,
        port,
        host_name: crate::mdns::HOST_NAME.to_string(),
    };
    if std::env::var("NO_TLS").is_ok() {
        data.properties
            .push(("ws".to_string(), format!("ws://{{IF_IP}}:{port}")));
    } else {
        data.properties
            .push(("ws".to_string(), format!("wss://{{IF_IP}}:{port}")));
    }

    let data_json = serde_json::to_string(&data)?;
    loop {
        _ = client
            .post(format!("{api_url}/mdns"))
            .body(data_json.clone())
            .header("Content-Type", "application/json")
            .send()
            .await;

        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

#[derive(Debug)]
pub struct BleAdapter {
    pub client: reqwest::Client,
    pub api_url: String,
}

#[derive(Debug, Deserialize)]
pub struct BleAdapterDevice {
    pub device_id: String,
    pub local_name: String,
}

#[derive(Debug, Serialize)]
struct BleScanApi {
    pub scan_timeout_ms: u64,
}

#[derive(Debug, Serialize)]
struct BleWriteApi {
    pub device_id: String,
    pub characteristic: Uuid,
    pub data: Vec<u8>,
}

impl BleAdapter {
    pub fn new(client: reqwest::Client, api_url: String) -> Self {
        Self { client, api_url }
    }

    pub async fn scan(&mut self, scan_timeout: Duration) -> Result<Vec<BleAdapterDevice>> {
        let data = BleScanApi {
            scan_timeout_ms: scan_timeout.as_millis() as u64,
        };

        let data_json = serde_json::to_string(&data)?;
        Ok(self
            .client
            .post(format!("{}/ble", self.api_url))
            .body(data_json.clone())
            .header("Content-Type", "application/json")
            .timeout(scan_timeout.mul_f64(2.0))
            .send()
            .await?
            .json::<Vec<BleAdapterDevice>>()
            .await?)
    }

    pub async fn write_to_device(
        &mut self,
        device_id: String,
        characteristic_uuid: Uuid,
        data: &[u8],
    ) -> Result<()> {
        let data = BleWriteApi {
            device_id,
            characteristic: characteristic_uuid,
            data: data.to_vec(),
        };
        let data_json = serde_json::to_string(&data)?;
        _ = self
            .client
            .put(format!("{}/ble", self.api_url))
            .body(data_json.clone())
            .header("Content-Type", "application/json")
            .send()
            .await?;

        Ok(())
    }
}
