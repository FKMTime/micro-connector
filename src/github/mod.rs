use anyhow::Result;

mod structs;

const GITHUB_API_URL: &str = "https://api.github.com";
const GH_OWNER: &str = "FKMTime";
const REPOS: &[&str] = &["firmware", "staff-at-firmware"];

pub async fn get_releases(client: &reqwest::Client) -> Result<Vec<structs::ReleaseAssetItem>> {
    let mut tmp = Vec::new();

    for repo in REPOS {
        let url = format!("{GITHUB_API_URL}/repos/{GH_OWNER}/{repo}/releases");

        let res = client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        let release: structs::GithubRelease = {
            let text = res.text().await?;

            let json: Vec<structs::GithubRelease> = serde_json::from_str(&text)?;
            json.iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("No releases found!"))?
                .to_owned()
        };

        tmp.extend(release.assets.iter().map(|x| structs::ReleaseAssetItem {
            name: x.name.to_string(),
            download_url: x.browser_download_url.to_string(),
        }));
    }

    Ok(tmp)
}
