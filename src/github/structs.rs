use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct ReleaseAssetItem {
    pub name: String,
    pub download_url: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRelease {
    pub url: String,

    #[serde(rename = "assets_url")]
    pub assets_url: String,

    pub id: i64,

    #[serde(rename = "tag_name")]
    pub tag_name: String,

    #[serde(rename = "target_commitish")]
    pub target_commitish: String,

    pub name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub url: String,
    pub id: i64,
    pub name: String,

    #[serde(rename = "content_type")]
    pub content_type: String,

    pub size: i64,

    #[serde(rename = "browser_download_url")]
    pub browser_download_url: String,
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubReleaseItem {
    pub name: String,
    pub tag: String,
    pub url: String,
}
