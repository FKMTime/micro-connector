use crate::structs::UpdateStrategy;
use anyhow::{Context, Result};

mod structs;

const GITHUB_API_URL: &str = "https://api.github.com";
const GH_OWNER: &str = "filipton";
const GH_REPO: &str = "fkm-timer";

pub async fn get_releases(
    client: &reqwest::Client,
    update_strategy: UpdateStrategy,
) -> Result<Vec<structs::ReleaseAssetItem>> {
    if update_strategy == UpdateStrategy::Disabled {
        return Err(anyhow::anyhow!("Udpdates disabled!"));
    }

    let url = match update_strategy {
        UpdateStrategy::Stable => {
            format!("{GITHUB_API_URL}/repos/{GH_OWNER}/{GH_REPO}/releases/latest")
        }
        UpdateStrategy::Prerelease => {
            format!("{GITHUB_API_URL}/repos/{GH_OWNER}/{GH_REPO}/releases")
        }
        UpdateStrategy::Disabled => {
            return Err(anyhow::anyhow!("Udpdates disabled!"));
        }
    };

    let res = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;

    let release: structs::GithubRelease = match update_strategy {
        UpdateStrategy::Stable => res
            .json()
            .await
            .context("No releases found or failed to parse latest release")?,
        UpdateStrategy::Prerelease => {
            let json: Vec<structs::GithubRelease> = res.json().await?;
            json.iter()
                .filter(|r| r.prerelease)
                .next()
                .ok_or_else(|| anyhow::anyhow!("No releases found!"))?
                .to_owned()
        }
        UpdateStrategy::Disabled => {
            return Err(anyhow::anyhow!("Udpdates disabled!"));
        }
    };

    Ok(release
        .assets
        .iter()
        .map(|x| structs::ReleaseAssetItem {
            name: x.name.to_string(),
            download_url: x.browser_download_url.to_string(),
        })
        .collect())
}
