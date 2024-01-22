use std::path::PathBuf;

use fastwebsockets::WebSocketError;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;

use crate::structs::TimerResponse;

/// Returns true if client was updated
pub async fn update_client(
    ws: &mut fastwebsockets::FragmentCollector<TokioIo<Upgraded>>,
    id: u128,
    version_time: u128,
    chip: &str,
) -> Result<bool, WebSocketError> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_firmware: (Option<PathBuf>, u128) = (None, version_time);
    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_split: Vec<&str> = file_name.to_str().unwrap().split('.').collect();

        if name_split.len() < 3 || name_split[0] != chip {
            continue;
        }

        let version = u128::from_str_radix(name_split[1], 16).unwrap();
        if version > latest_firmware.1 {
            latest_firmware = (Some(entry.path()), version);
        }
    }

    if latest_firmware.0.is_none() || version_time >= latest_firmware.1 {
        return Ok(false);
    }

    println!("Updating client to version {:x}", latest_firmware.1);
    println!("Firmware file: {:?}", latest_firmware.0);
    let firmware_file = tokio::fs::read(latest_firmware.0.unwrap()).await?;
    let frame = fastwebsockets::Frame::text(
        serde_json::to_vec(&TimerResponse::StartUpdate {
            esp_id: id,
            version: format!("{:x}", latest_firmware.1),
            size: firmware_file.len() as i64,
        })
        .unwrap()
        .into(),
    );
    ws.write_frame(frame).await?;

    // 1s delay to allow esp to process
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let chunk_size = 1024 * 4;
    let mut firmware_chunks = firmware_file.chunks(chunk_size);

    while let Some(chunk) = firmware_chunks.next() {
        let frame = fastwebsockets::Frame::binary(chunk.into());
        ws.write_frame(frame).await?;

        if firmware_chunks.len() % 10 == 0 {
            println!(
                "[{}] {}/{} chunks left",
                id,
                firmware_chunks.len(),
                firmware_file.len() / chunk_size
            );
        }

        if firmware_chunks.len() == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Wait for esp to process
                                                                         // (important)

            break;
        }

        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), ws.read_frame())
            .await
            .or_else(|_| {
                println!("Timeout while updating");
                Err(WebSocketError::ConnectionClosed)
            })?;

        let frame = frame?;
        if frame.opcode == fastwebsockets::OpCode::Close {
            return Ok(false);
        }
    }

    Ok(true)
}
