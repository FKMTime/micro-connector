use crate::{http::EspConnectInfo, structs::TimerResponse};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use std::path::PathBuf;
use tracing::{debug, error, info};

const UPDATE_CHUNK_SIZE: usize = 1024 * 4;

/// Returns true if client was updated
pub async fn update_client(
    socket: &mut WebSocket,
    esp_connect_info: &EspConnectInfo,
) -> Result<bool> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let eci_build_time = u128::from_str_radix(&esp_connect_info.build_time, 16)?;
    let mut latest_firmware: (Option<PathBuf>, u128, String) =
        (None, eci_build_time, String::new());

    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_split: Vec<&str> = file_name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("file_name is none"))?
            .split('.')
            .collect();

        if name_split.len() != 4 || name_split[0] != esp_connect_info.chip {
            continue;
        }

        let version = name_split[1].to_string();
        let build_time = u128::from_str_radix(name_split[2], 16)?;
        if build_time > latest_firmware.1 {
            latest_firmware = (Some(entry.path()), build_time, version);
        }
    }

    if latest_firmware.0.is_none()
        || eci_build_time >= latest_firmware.1
        || latest_firmware.2 == esp_connect_info.version
    {
        return Ok(false);
    }

    info!(
        "Updating client from version: {} to version {}",
        esp_connect_info.version, latest_firmware.2
    );
    debug!("Firmware file: {:?}", latest_firmware.0);

    let firmware_file = tokio::fs::read(latest_firmware.0.expect("Cant be none")).await?;
    let start_update_resp = TimerResponse::StartUpdate {
        esp_id: esp_connect_info.id,
        version: latest_firmware.2,
        build_time: latest_firmware.1,
        size: firmware_file.len() as i64,
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

    let mut firmware_chunks = firmware_file.chunks(UPDATE_CHUNK_SIZE);

    while let Some(chunk) = firmware_chunks.next() {
        let msg = Message::Binary(chunk.to_vec());
        socket.send(msg).await?;

        if firmware_chunks.len() % 10 == 0 {
            debug!(
                "[{}] {}/{} chunks left",
                esp_connect_info.id,
                firmware_chunks.len(),
                firmware_file.len() / UPDATE_CHUNK_SIZE
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
