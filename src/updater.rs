use crate::{http::EspConnectInfo, structs::TimerPacket};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use std::{fmt::Write, ops::RangeBounds, path::PathBuf, time::SystemTime};
use tracing::{debug, error, info};

const UPDATE_CHUNK_SIZE: usize = 1024 * 4;

#[derive(Debug)]
pub struct Firmware {
    pub data: Vec<u8>,
    pub version: String,
    pub build_time: u64,
    pub firmware: String,
}

pub async fn should_update(esp_connect_info: &EspConnectInfo) -> Result<Option<Firmware>> {
    let firmware_dir = std::env::var("FIRMWARE_DIR")?;
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_firmware: (Option<PathBuf>, String, String, SystemTime) =
        (None, String::new(), String::new(), SystemTime::UNIX_EPOCH);

    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        let modified = entry.metadata()?.modified()?;
        let path = entry.path();
        let file_name = path.file_stem();
        let name_split: Vec<&str> = file_name
            .ok_or_else(|| anyhow::anyhow!("file_name is none"))?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("file_name is none"))?
            .split('_')
            .collect();

        if name_split.len() != 3 {
            continue;
        }

        let (chip, firmware, version) = (name_split[0], name_split[1], name_split[2]);
        if chip != esp_connect_info.chip || firmware != esp_connect_info.firmware {
            continue;
        }

        if version != latest_firmware.1 && modified > latest_firmware.3 {
            latest_firmware = (
                Some(entry.path()),
                version.to_string(),
                firmware.to_string(),
                modified,
            );
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

pub async fn update_client(
    socket: &mut WebSocket,
    esp_connect_info: &EspConnectInfo,
    latest_firmware: Firmware,
) -> Result<bool> {
    info!(
        "[{}] Updating client from version: {} to version {}",
        esp_connect_info.firmware, esp_connect_info.version, latest_firmware.version
    );

    let start_update_resp = TimerPacket::StartUpdate {
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

/// Inner u128 is calculated version number,
/// if version is newer, the number should be bigger
/// * Important: you can't compare between Dev and Stable version!
#[derive(PartialEq)]
pub enum Version {
    /// Like v2.1.0
    Stable(u128),

    /// Like DV542356675 (epoch build time)
    Dev(u128),

    /// Any other version string
    Other,
}

impl Version {
    pub fn from_str(string: &str) -> Self {
        if string.starts_with("v") {
            let nmb = Self::read_stable_version_nmb(string.trim_start_matches('v'));
            Self::Stable(nmb)
        } else if string.starts_with("DV") {
            let nmb: u128 = string.trim_start_matches("DV").parse().unwrap_or(0);
            Self::Dev(nmb)
        } else {
            Self::Other
        }
    }

    /// Check if version provided as input is newer than self
    pub fn is_version_newer(&self, ver: Self) -> bool {
        if ver == Version::Other {
            return false;
        }

        // Only compare if version variants are the same!
        if matches!(ver, Version::Stable(..)) && matches!(self, Version::Stable(..)) {
            return ver.get_nmb() > self.get_nmb();
        } else if matches!(ver, Version::Dev(..)) && matches!(self, Version::Dev(..)) {
            return ver.get_nmb() > self.get_nmb();
        } else {
            true
        }
    }

    fn get_nmb(&self) -> u128 {
        match self {
            &Self::Stable(d) => d,
            &Self::Dev(d) => d,
            &Self::Other => 0,
        }
    }

    fn read_stable_version_nmb(string: &str) -> u128 {
        let mut tmp = 0;
        let mut mult = 1;

        for c in string.chars().rev() {
            if let Some(d) = c.to_digit(10) {
                tmp += mult * d as u128;
                mult *= 10;
            }
        }

        tmp
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            &Version::Stable(nmb) => f.write_fmt(format_args!("Version::Stable({})", nmb)),
            &Version::Dev(nmb) => f.write_fmt(format_args!("Version::Dev({})", nmb)),
            &Version::Other => f.write_str("Version::Other"),
        }
    }
}
