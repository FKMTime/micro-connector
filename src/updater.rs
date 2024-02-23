use crate::structs::TimerResponse;
use anyhow::Result;
use fastwebsockets::WebSocketError;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use std::path::PathBuf;

const UPDATE_CHUNK_SIZE: usize = 1024 * 4;

/// Returns true if client was updated
pub async fn update_client(
    ws: &mut fastwebsockets::FragmentCollector<TokioIo<Upgraded>>,
    id: u128,
    version: &str,
    build_time: u128,
    chip: &str,
) -> Result<bool, WebSocketError> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_firmware: (Option<PathBuf>, u128, String) = (None, build_time, String::new());
    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_split: Vec<&str> = file_name.to_str().unwrap().split('.').collect();

        if name_split.len() != 4 || name_split[0] != chip {
            continue;
        }

        let version = name_split[1].to_string();
        let build_time = u128::from_str_radix(name_split[2], 16).unwrap();
        if build_time > latest_firmware.1 {
            latest_firmware = (Some(entry.path()), build_time, version);
        }
    }

    if latest_firmware.0.is_none()
        || build_time >= latest_firmware.1
        || latest_firmware.2 == version
    {
        return Ok(false);
    }

    println!(
        "Updating client from version: {} to version {}",
        version, latest_firmware.2
    );
    println!("Firmware file: {:?}", latest_firmware.0);

    let firmware_file = tokio::fs::read(latest_firmware.0.unwrap()).await?;
    let frame = fastwebsockets::Frame::text(
        serde_json::to_vec(&TimerResponse::StartUpdate {
            esp_id: id,
            version: latest_firmware.2,
            build_time: latest_firmware.1,
            size: firmware_file.len() as i64,
        })
        .unwrap()
        .into(),
    );
    ws.write_frame(frame).await?;

    // wait for esp to respond
    tokio::time::timeout(std::time::Duration::from_secs(10), ws.read_frame())
        .await
        .or_else(|_| {
            println!("Timeout while updating");
            Err(WebSocketError::ConnectionClosed)
        })??;

    let mut firmware_chunks = firmware_file.chunks(UPDATE_CHUNK_SIZE);

    while let Some(chunk) = firmware_chunks.next() {
        let frame = fastwebsockets::Frame::binary(chunk.into());
        ws.write_frame(frame).await?;

        if firmware_chunks.len() % 10 == 0 {
            println!(
                "[{}] {}/{} chunks left",
                id,
                firmware_chunks.len(),
                firmware_file.len() / UPDATE_CHUNK_SIZE
            );
        }

        if firmware_chunks.len() == 0 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await; // Wait for esp to process
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

pub async fn spawn_build_watcher(broadcaster: tokio::sync::broadcast::Sender<()>) -> Result<()> {
    tokio::task::spawn(async move {
        loop {
            let res = build_watcher(&broadcaster).await;
            if let Err(e) = res {
                println!("Error in build watcher: {:?}", e);
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });

    Ok(())
}

async fn build_watcher(broadcaster: &tokio::sync::broadcast::Sender<()>) -> Result<()> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_modified: u128 = 0;

    loop {
        let mut modified_state = false;
        for entry in firmware_dir.read_dir()? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                continue;
            }

            let modified = entry
                .metadata()?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis();

            if modified > latest_modified {
                latest_modified = modified;
                modified_state = true;
            }
        }

        if modified_state {
            _ = broadcaster.send(());
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
