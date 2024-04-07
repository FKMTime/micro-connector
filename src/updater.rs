use crate::{
    http::EspConnectInfo,
    structs::{ReleaseChannel, TimerResponse},
    FIRMWARE_CACHE,
};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use serde::Deserialize;
use std::path::PathBuf;
use tracing::{debug, error, info};

const UPDATE_CHUNK_SIZE: usize = 1024 * 4;

#[derive(Debug)]
pub struct Firmware {
    pub data: Vec<u8>,
    pub version: String,
    pub build_time: u64,
    pub firmware: String,
}

pub async fn should_update(
    esp_connect_info: &EspConnectInfo,
    channel: ReleaseChannel,
) -> Result<Option<Firmware>> {
    let dev_mode = crate::DEV_MODE
        .get()
        .ok_or_else(|| anyhow::anyhow!("DEV_MODE not set"))?;

    if *dev_mode {
        should_update_dev_mode(esp_connect_info).await
    } else {
        should_update_ota(esp_connect_info, channel).await
    }
}

async fn should_update_dev_mode(esp_connect_info: &EspConnectInfo) -> Result<Option<Firmware>> {
    let firmware_dir = std::env::var("FIRMWARE_DIR")?;
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_firmware: (Option<PathBuf>, u128, String) = (None, 0, String::new());

    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_split: Vec<&str> = file_name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("file_name is none"))?
            .split('.')
            .collect();

        if name_split.len() != 4
            || name_split[0] != esp_connect_info.chip
            || name_split[1] != esp_connect_info.firmware
        {
            continue;
        }

        let version = name_split[2].to_string();
        let firmware = name_split[0].to_string();

        let version: u128 = version.parse().unwrap_or(0);
        if version > latest_firmware.1 {
            latest_firmware = (Some(entry.path()), version, firmware);
        }
    }

    if latest_firmware.0.is_none() || latest_firmware.1.to_string() == esp_connect_info.version {
        return Ok(None);
    }

    Ok(Some(Firmware {
        data: tokio::fs::read(latest_firmware.0.expect("Cant be none")).await?,
        version: latest_firmware.1.to_string(),
        build_time: 0,
        firmware: latest_firmware.2,
    }))
}

const OTA_URL: &str = "https://ota.filipton.space";

#[derive(Debug, Deserialize)]
struct OtaLatestFirmware {
    version: String,
    file: String,

    #[serde(rename = "buildTime")]
    build_time: u64,
}

async fn should_update_ota(
    esp_connect_info: &EspConnectInfo,
    channel: ReleaseChannel,
) -> Result<Option<Firmware>> {
    let (client, _) = crate::api::ApiClient::get_api_client()?;

    let channel = match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Prerelease => "prerelease",
    };

    let url = format!(
        "{OTA_URL}/firmware/{}/{}/{}/latest.json",
        esp_connect_info.firmware, channel, esp_connect_info.chip
    );

    let res = client.get(url).send().await?;
    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await?;

    if !success {
        tracing::error!("Ota response not success ({status_code}): {text}");
        return Err(anyhow::anyhow!("Ota response not success"));
    }

    tracing::trace!("Ota response: {text}");

    let json: OtaLatestFirmware = serde_json::from_str(&text)?;
    if json.build_time > esp_connect_info.build_time {
        let mut firmware_cache = FIRMWARE_CACHE.get().expect("Should be set").lock().await;
        let (cache_ver, mut cache_bytes) =
            firmware_cache.clone().unwrap_or(("".to_string(), vec![]));

        if firmware_cache.is_none() || json.version != cache_ver {
            let firmware_url = format!("{OTA_URL}/{}", json.file);
            let firmware_bytes = client
                .get(firmware_url)
                .send()
                .await?
                .bytes()
                .await?
                .to_vec();

            tracing::trace!(
                "Downloaded firmware with version: {}, Size: {}!",
                json.version,
                firmware_bytes.len()
            );

            cache_bytes = firmware_bytes.clone();
            *firmware_cache = Some((json.version.clone(), firmware_bytes));
        }

        tracing::trace!("dsa");
        return Ok(Some(Firmware {
            data: cache_bytes,
            version: json.version,
            build_time: json.build_time,
            firmware: esp_connect_info.firmware.clone(),
        }));
    }

    Ok(None)
}

pub async fn update_client(
    socket: &mut WebSocket,
    esp_connect_info: &EspConnectInfo,
    latest_firmware: Firmware,
) -> Result<bool> {
    info!(
        "[{}] Updating client from version: {} to version {}",
        esp_connect_info.firmware, esp_connect_info.version, latest_firmware.version
    );

    let start_update_resp = TimerResponse::StartUpdate {
        esp_id: esp_connect_info.id,
        version: latest_firmware.version,
        build_time: latest_firmware.build_time,
        size: latest_firmware.data.len() as i64,
        firmware: latest_firmware.firmware,
    };

    socket
        .send(Message::Text(serde_json::to_string(&start_update_resp)?))
        .await?;

    // wait for esp to respond
    tokio::time::timeout(std::time::Duration::from_secs(10), socket.recv())
        .await
        .or_else(|_| {
            error!("Timeout while updating");
            Err(anyhow::anyhow!("Timeout while updating"))
        })?;

    let mut firmware_chunks = latest_firmware.data.chunks(UPDATE_CHUNK_SIZE);

    while let Some(chunk) = firmware_chunks.next() {
        let msg = Message::Binary(chunk.to_vec());
        socket.send(msg).await?;

        if firmware_chunks.len() % 10 == 0 {
            debug!(
                "[{}] {}/{} chunks left",
                esp_connect_info.id,
                firmware_chunks.len(),
                latest_firmware.data.len() / UPDATE_CHUNK_SIZE
            );
        }

        if firmware_chunks.len() == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await; // Wait for esp to process
                                                                         // (important)

            break;
        }

        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), socket.recv())
            .await
            .or_else(|_| {
                error!("Timeout while updating");
                Err(anyhow::anyhow!("Timeout while updating"))
            })?;

        let frame = frame.ok_or_else(|| anyhow::anyhow!("Frame option is none"))??;
        match frame {
            Message::Close(_) => {
                return Ok(false);
            }
            _ => {}
        }
    }

    Ok(true)
}
