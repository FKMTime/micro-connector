use crate::structs::SharedCompetitionStatus;
use anyhow::Result;
use std::{path::PathBuf, time::Duration};
use tracing::error;

const GITHUB_UPDATE_INTERVAL: u64 = 90000;
const UPDATE_STRATEGY_INTERVAL: u64 = 15000;

pub async fn spawn_watchers(
    broadcaster: tokio::sync::broadcast::Sender<()>,
    device_settings_broadcaster: tokio::sync::broadcast::Sender<()>,
    comp_status: SharedCompetitionStatus,
) -> Result<()> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let (client, api_url) = crate::api::ApiClient::get_api_client()?;

    let mut build_interval = tokio::time::interval(Duration::from_secs(1));
    let mut github_releases_interval =
        tokio::time::interval(Duration::from_millis(GITHUB_UPDATE_INTERVAL));
    let mut comp_status_interval =
        tokio::time::interval(Duration::from_millis(UPDATE_STRATEGY_INTERVAL));

    tokio::task::spawn(async move {
        loop {
            tokio::select! {
                _ = build_interval.tick() => {
                    let res = build_watcher(&broadcaster, &firmware_dir).await;
                    if let Err(e) = res {
                        error!("Error in build watcher: {:?}", e);
                    }
                }
                _ = github_releases_interval.tick() => {
                    if !comp_status.read().await.should_update {
                        continue;
                    }

                    let res = github_releases_watcher(&client, &firmware_dir, &comp_status).await;
                    if let Err(e) = res {
                        error!("Error in github releases watcher: {:?}", e);
                    }
                }
                _ = comp_status_interval.tick() => {
                    let res = comp_status_watcher(&client, &api_url, &comp_status, &device_settings_broadcaster).await;
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

async fn github_releases_watcher(
    client: &reqwest::Client,
    firmware_dir: &PathBuf,
    comp_status: &SharedCompetitionStatus,
) -> Result<()> {
    let releases = crate::github::get_releases(client, comp_status).await?;

    for release in releases {
        let release_path = firmware_dir.join(&release.name);
        if let Ok(exists) = tokio::fs::try_exists(&release_path).await {
            if !exists {
                let resp = client.get(&release.download_url).send().await?;
                tokio::fs::write(&release_path, resp.bytes().await?).await?;
            }
        }
    }

    Ok(())
}

async fn comp_status_watcher(
    client: &reqwest::Client,
    api_url: &str,
    comp_status: &SharedCompetitionStatus,
    device_settings_broadcaster: &tokio::sync::broadcast::Sender<()>,
) -> Result<()> {
    let comp_status_res = crate::api::get_competition_status(client, api_url).await?;
    let mut comp_status = comp_status.write().await;

    comp_status.should_update = comp_status_res.should_update;
    comp_status.release_channel = comp_status_res.release_channel;

    let mut changed = false;

    // delete devices that are not in the new status
    let devices_clone = comp_status.devices_settings.clone();
    for (k, _) in devices_clone {
        if !comp_status_res.rooms.iter().any(|r| r.devices.contains(&k)) {
            comp_status.devices_settings.remove(&k);
            changed = true;
        }
    }

    for room in comp_status_res.rooms {
        for device in room.devices {
            let old = comp_status.devices_settings.insert(
                device,
                crate::structs::CompetitionDeviceSettings {
                    use_inspection: room.use_inspection,
                },
            );

            if old.is_none() || old.unwrap().use_inspection != room.use_inspection {
                changed = true;
            }
        }
    }

    if changed {
        _ = device_settings_broadcaster.send(());
    }
    Ok(())
}
