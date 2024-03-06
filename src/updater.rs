use crate::structs::{GithubReleaseItem, TimerResponse, UpdateStrategy};
use anyhow::{anyhow, Result};
use fastwebsockets::WebSocketError;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use std::{path::PathBuf, time::Duration};
use tokio::select;
use tracing::{debug, error, info};

const UPDATE_CHUNK_SIZE: usize = 1024 * 4;
const GITHUB_UPDATE_INTERVAL: u64 = 30000;
const UPDATE_STRATEGY_INTERVAL: u64 = 60000;
const GRM_URL: &str = "https://grm.filipton.space";
const GH_OWNER: &str = "filipton";
const GH_REPO: &str = "fkm-timer";

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

    info!(
        "Updating client from version: {} to version {}",
        version, latest_firmware.2
    );
    debug!("Firmware file: {:?}", latest_firmware.0);

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
            error!("Timeout while updating");
            Err(WebSocketError::ConnectionClosed)
        })??;

    let mut firmware_chunks = firmware_file.chunks(UPDATE_CHUNK_SIZE);

    while let Some(chunk) = firmware_chunks.next() {
        let frame = fastwebsockets::Frame::binary(chunk.into());
        ws.write_frame(frame).await?;

        if firmware_chunks.len() % 10 == 0 {
            debug!(
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
                error!("Timeout while updating");
                Err(WebSocketError::ConnectionClosed)
            })?;

        let frame = frame?;
        if frame.opcode == fastwebsockets::OpCode::Close {
            return Ok(false);
        }
    }

    Ok(true)
}

pub async fn spawn_watchers(broadcaster: tokio::sync::broadcast::Sender<()>) -> Result<()> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let (client, api_url) = crate::api::ApiClient::get_api_client()?;

    let mut build_interval = tokio::time::interval(Duration::from_secs(1));
    let mut github_releases_interval =
        tokio::time::interval(Duration::from_millis(GITHUB_UPDATE_INTERVAL));
    let mut update_strategy_interval =
        tokio::time::interval(Duration::from_millis(UPDATE_STRATEGY_INTERVAL));

    tokio::task::spawn(async move {
        loop {
            select! {
                _ = build_interval.tick() => {
                    let res = build_watcher(&broadcaster, &firmware_dir).await;
                    if let Err(e) = res {
                        error!("Error in build watcher: {:?}", e);
                    }
                }
                _ = github_releases_interval.tick() => {
                    if !UpdateStrategy::should_update() {
                        continue;
                    }

                    let res = github_releases_watcher(&client, &firmware_dir).await;
                    if let Err(e) = res {
                        error!("Error in github releases watcher: {:?}", e);
                    }
                }
                _ = update_strategy_interval.tick() => {
                    let res = update_strategy_watcher(&client, &api_url).await;
                    if let Err(e) = res {
                        error!("Error in update strategy watcher: {:?}", e);
                    }
                }
            }
        }
    });

    Ok(())
}

async fn build_watcher(
    broadcaster: &tokio::sync::broadcast::Sender<()>,
    firmware_dir: &PathBuf,
) -> Result<()> {
    let mut latest_modified: u128 = 0;

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

    Ok(())
}

async fn github_releases_watcher(client: &reqwest::Client, firmware_dir: &PathBuf) -> Result<()> {
    let tag = match UpdateStrategy::get() {
        UpdateStrategy::Disabled => return Ok(()),
        UpdateStrategy::Stable => "latest",
        UpdateStrategy::Prerelease => "prerelease",
    };

    let url = format!("{GRM_URL}/releases/{GH_OWNER}/{GH_REPO}/{tag}");
    let resp = client.get(&url).send().await?;
    let resp: Vec<GithubReleaseItem> = resp.json().await?;

    if resp.len() == 0 {
        return Err(anyhow!("No releases"));
    }

    let github_release = resp
        .first()
        .ok_or_else(|| anyhow!("Rather impossible but no releases found."))?;

    let release_path = firmware_dir.join(&github_release.name);
    if let Ok(exists) = tokio::fs::try_exists(&release_path).await {
        if !exists {
            let resp = client.get(&github_release.url).send().await?;
            tokio::fs::write(release_path, resp.bytes().await?).await?;
        }
    }

    Ok(())
}

async fn update_strategy_watcher(client: &reqwest::Client, api_url: &str) -> Result<()> {
    let should_update = crate::api::should_update_devices(client, api_url).await?;
    let update_strategy = match should_update {
        (true, true) => UpdateStrategy::Stable,
        (true, false) => UpdateStrategy::Prerelease,
        (false, _) => UpdateStrategy::Disabled,
    };

    UpdateStrategy::set(update_strategy);
    Ok(())
}
