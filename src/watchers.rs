use crate::structs::SharedAppState;
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::error;

const GITHUB_UPDATE_INTERVAL: u64 = 60000 * 5;

pub async fn spawn_watchers(state: SharedAppState) -> Result<()> {
    let firmware_dir = std::env::var("FIRMWARE_DIR").expect("FIRMWARE_DIR not set");
    let firmware_dir = std::path::PathBuf::from(firmware_dir);

    let mut dev_build_interval = tokio::time::interval(Duration::from_secs(1));
    let mut github_releases_interval =
        tokio::time::interval(Duration::from_millis(GITHUB_UPDATE_INTERVAL));

    let mut last_build_modified: u128 = 0;
    tokio::task::spawn(async move {
        loop {
            tokio::select! {
                _ = dev_build_interval.tick() => {
                    let res = dev_build_watcher(&state, &firmware_dir, &mut last_build_modified).await;
                    if let Err(e) = res {
                        error!("Error in build watcher: {:?}", e);
                    }
                }
                _ = github_releases_interval.tick() => {
                    let res = github_releases_watcher(&state, &firmware_dir).await;
                    if let Err(e) = res {
                        error!("Error in github releases watcher: {:?}", e);
                    }
                }
            }
        }
    });

    Ok(())
}

async fn dev_build_watcher(
    state: &SharedAppState,
    firmware_dir: &Path,
    last_modified: &mut u128,
) -> Result<()> {
    if !state.dev_mode {
        return Ok(());
    }

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

        if modified > *last_modified {
            *last_modified = modified;
            modified_state = true;
        }
    }

    if modified_state {
        _ = state.build_broadcast().await;
    }

    Ok(())
}

async fn github_releases_watcher(state: &SharedAppState, firmware_dir: &Path) -> Result<()> {
    let client = reqwest::Client::builder().user_agent("Fkm/2.0").build()?;
    let files = crate::github::get_releases(&client).await?;

    for file in files {
        let release_path = firmware_dir.join(&file.name);
        let tmp_path = PathBuf::from("/tmp").join(&file.name);
        if let Ok(exists) = tokio::fs::try_exists(&release_path).await
            && !exists
        {
            let resp = client.get(&file.download_url).send().await?;
            tokio::fs::write(&tmp_path, resp.bytes().await?).await?;
            move_file(&tmp_path, &release_path).await?;

            tracing::info!("Downloaded new release: {}", file.name);
            _ = state.build_broadcast().await;
        }
    }

    Ok(())
}

async fn move_file(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()> {
    if let Err(e) = tokio::fs::rename(&src, &dest).await {
        if e.kind().to_string().contains("cross-device link or rename") {
            tokio::fs::copy(&src, &dest).await?;
            tokio::fs::remove_file(&src).await?;
        } else {
            return Err(e.into());
        }
    }
    Ok(())
}
