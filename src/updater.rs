use crate::{
    http::EspConnectInfo,
    structs::{SharedAppState, TimerPacket, TimerPacketInner},
};
use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use std::path::PathBuf;
use tracing::{debug, error, info};

const UPDATE_CHUNK_SIZE: usize = 1024 * 4;

#[derive(Debug)]
pub struct Firmware {
    pub data: Vec<u8>,
    pub version: Version,
    pub build_time: u64,
    pub firmware: String,
}

pub async fn should_update(
    state: &SharedAppState,
    esp_connect_info: &EspConnectInfo,
) -> Result<Option<Firmware>> {
    let firmware_dir = std::env::var("FIRMWARE_DIR")?;
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut latest_firmware: (Option<PathBuf>, Version, String) = (
        None,
        Version::from_str(&esp_connect_info.version),
        String::new(),
    );

    for entry in firmware_dir.read_dir()? {
        let entry = entry?;
        //let modified = entry.metadata()?.modified()?;
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

        let (hw, firmware, version) = (
            name_split[0],
            name_split[1],
            Version::from_str(name_split[2]),
        );

        if hw != esp_connect_info.hw || firmware != esp_connect_info.firmware {
            continue;
        }

        if (state.dev_mode && version.is_stable()) || (!state.dev_mode && version.is_dev()) {
            continue;
        }

        if latest_firmware.1.is_newer(&version) {
            latest_firmware = (Some(entry.path()), version, firmware.to_string());
        }
    }

    if latest_firmware.0.is_none() {
        return Ok(None);
    }

    Ok(Some(Firmware {
        data: tokio::fs::read(latest_firmware.0.expect("Cant be none")).await?,
        version: latest_firmware.1,
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
        esp_connect_info.firmware,
        Version::from_str(&esp_connect_info.version),
        latest_firmware.version
    );

    let crc = crc32fast::hash(&latest_firmware.data);
    let start_update_resp = TimerPacket {
        tag: None,
        data: TimerPacketInner::StartUpdate {
            version: latest_firmware.version.inner_version(),
            build_time: latest_firmware.build_time,
            size: latest_firmware.data.len() as u32,
            crc,
            firmware: latest_firmware.firmware,
        },
    };

    socket
        .send(Message::Text(
            serde_json::to_string(&start_update_resp)?.into(),
        ))
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
        let msg = Message::Binary(chunk.to_vec().into());
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
#[derive(Debug, PartialEq)]
pub enum Version {
    /// Like v2.1.0
    Stable(String),

    /// Like DV542356675 (epoch build time)
    Dev(u128),

    /// Any other version string
    Other,
}

impl Version {
    pub fn from_str(string: &str) -> Self {
        if string.starts_with("v") {
            Self::Stable(string.trim_start_matches('v').to_string())
        } else if string.starts_with('D') {
            let nmb = string.trim_start_matches('D').parse().unwrap_or(0);
            Self::Dev(nmb)
        } else {
            Self::Other
        }
    }

    /// Check if version provided as input is newer than self
    pub fn is_newer(&self, ver: &Self) -> bool {
        if ver == &Version::Other {
            return false;
        }

        // Only compare if version variants are the same!
        if matches!(ver, Version::Stable(..)) && matches!(self, Version::Stable(..)) {
            return Self::compare_stable(&self.get_str(), &ver.get_str());
        } else if matches!(ver, Version::Dev(..)) && matches!(self, Version::Dev(..)) {
            return ver.get_nmb() > self.get_nmb();
        } else {
            true
        }
    }

    pub fn is_stable(&self) -> bool {
        if let Self::Stable(_) = self {
            return true;
        }

        false
    }

    pub fn is_dev(&self) -> bool {
        if let Self::Dev(_) = self {
            return true;
        }

        false
    }

    /// return if v2 is newer than v1 (for stable only)
    fn compare_stable(v1: &str, v2: &str) -> bool {
        let v1 = v1.split('.').collect::<Vec<_>>();
        let v2 = v2.split('.').collect::<Vec<_>>();

        let mut same_beg = true;
        for i in 0..v1.len() {
            if let Some(v2) = v2.get(i) {
                if v1[i] != *v2 {
                    same_beg = false;
                }

                let v1 = v1[i].parse().unwrap_or(-1);
                let v2 = v2.parse().unwrap_or(-1);

                if v2 > v1 {
                    return true;
                } else if v2 < v1 {
                    return false;
                }
            } else {
                break;
            }
        }

        // if same beggining and the second one is longer (dot parts)
        if same_beg && v2.len() > v1.len() {
            return true;
        }

        false
    }

    /// Only used for dev comparison
    fn get_nmb(&self) -> u128 {
        match self {
            Self::Dev(d) => *d,
            _ => 0,
        }
    }

    /// Only used for stable comparison
    fn get_str(&self) -> String {
        match self {
            Self::Stable(s) => s.to_string(),
            _ => "".to_string(),
        }
    }

    pub fn inner_version(&self) -> String {
        match self {
            Self::Stable(s) => s.to_string(),
            Self::Dev(nmb) => nmb.to_string(),
            Self::Other => String::from("Other"),
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            &Version::Stable(ref str) => f.write_fmt(format_args!("Version::Stable({})", str)),
            &Version::Dev(nmb) => f.write_fmt(format_args!("Version::Dev({})", nmb)),
            &Version::Other => f.write_str("Version::Other"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn check() {
        assert_eq!(
            crate::updater::Version::Other.is_newer(&crate::updater::Version::from_str("v2.1.0")),
            true
        );

        assert_eq!(
            crate::updater::Version::Other.is_newer(&crate::updater::Version::from_str("DV1321")),
            true
        );

        assert_eq!(
            crate::updater::Version::from_str("DV1714320292")
                .is_newer(&crate::updater::Version::from_str("v2.1.0")),
            true
        );

        assert_eq!(
            crate::updater::Version::from_str("DV1714320292")
                .is_newer(&crate::updater::Version::from_str("DV1714320295")),
            false
        );

        assert_eq!(
            crate::updater::Version::from_str("DV1714320292")
                .is_newer(&crate::updater::Version::from_str("DV1714320291")),
            false
        );

        assert_eq!(
            crate::updater::Version::from_str("v2.1")
                .is_newer(&crate::updater::Version::from_str("v2.1.0")),
            true
        );

        assert_eq!(
            crate::updater::Version::from_str("v2.1.0")
                .is_newer(&crate::updater::Version::from_str("v2.1.12")),
            true
        );

        assert_eq!(
            crate::updater::Version::from_str("v2.0.1")
                .is_newer(&crate::updater::Version::from_str("v2.0.0")),
            false
        );

        assert_eq!(
            crate::updater::Version::from_str("v2.0.0")
                .is_newer(&crate::updater::Version::from_str("v2.0.0")),
            false
        );

        assert_eq!(
            crate::updater::Version::from_str("v2.2.0")
                .is_newer(&crate::updater::Version::from_str("v2.1.2")),
            false
        );

        assert_eq!(
            crate::updater::Version::from_str("v2.1.2")
                .is_newer(&crate::updater::Version::from_str("v2.2.0")),
            true
        );
    }
}
