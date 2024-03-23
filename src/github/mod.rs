use anyhow::{Context, Result};

mod structs;

const GITHUB_API_URL: &str = "https://api.github.com";
const GH_OWNER: &str = "filipton";
const GH_REPO: &str = "fkm-timer";

pub async fn get_releases(
    client: &reqwest::Client,
    comp_status: &crate::structs::SharedCompetitionStatus,
) -> Result<Vec<structs::ReleaseAssetItem>> {
    let comp_status = comp_status.read().await;

    if !comp_status.should_update {
        return Err(anyhow::anyhow!("Updates disabled!"));
    }

    let url = match comp_status.release_channel {
        crate::structs::ReleaseChannel::Stable => {
            format!("{GITHUB_API_URL}/repos/{GH_OWNER}/{GH_REPO}/releases/latest")
        }
        crate::structs::ReleaseChannel::Prerelease => {
            format!("{GITHUB_API_URL}/repos/{GH_OWNER}/{GH_REPO}/releases")
        }
    };

    let res = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;

    let release: structs::GithubRelease = match comp_status.release_channel {
        crate::structs::ReleaseChannel::Stable => res
            .json()
            .await
            .context("No releases found or failed to parse latest release")?,
        crate::structs::ReleaseChannel::Prerelease => {
            let json: Vec<structs::GithubRelease> = res.json().await?;
            json.iter()
                .filter(|r| r.prerelease)
                .next()
                .ok_or_else(|| anyhow::anyhow!("No releases found!"))?
                .to_owned()
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
