use anyhow::Result;
use tokio::sync::OnceCell;
use tracing::{error, trace};

static API_CLIENT: OnceCell<reqwest::Client> = OnceCell::const_new();

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorInfo {
    pub id: i64,
    pub registrant_id: Option<i64>,
    pub name: String,
    pub wca_id: Option<String>,
    pub country_iso2: Option<String>,
    pub gender: String,
    pub can_compete: bool,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorRes {
    pub message: String,
    pub should_reset_time: bool,
}

pub async fn get_competitor_info(card_id: u128) -> Result<CompetitorInfo, ApiErrorRes> {
    let client = API_CLIENT
        .get_or_init(|| async {
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .user_agent("FKM-Timer/0.1")
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap()
        })
        .await;

    let url = format!("{}/person/card/{}", crate::API_URL.get().unwrap(), card_id);
    let res = client.get(&url).send().await.map_err(|e| {
        error!("Error getting competitor info (send error): {}", e);

        ApiErrorRes {
            message: format!("Error getting competitor info"),
            should_reset_time: false,
        }
    })?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    trace!("Competitor info response (SC: {status_code}): {}", text);

    if !success {
        error!("Error getting competitor info (not success): {}", text);

        return Err(
            serde_json::from_str::<ApiErrorRes>(&text).map_err(|_| ApiErrorRes {
                message: format!("Error parsing error message"),
                should_reset_time: false,
            })?,
        );
    }

    let info = serde_json::from_str::<CompetitorInfo>(&text).map_err(|_| ApiErrorRes {
        message: format!("Error parsing competitor info"),
        should_reset_time: false,
    })?;
    Ok(info)
}

pub async fn send_solve_entry(
    time: u128,
    penalty: i64,
    solved_at: u128,
    esp_id: u128,
    judge_id: u128,
    competitor_id: u128,
    is_delegate: bool,
) -> Result<(), ApiErrorRes> {
    let client = API_CLIENT
        .get_or_init(|| async {
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .user_agent("FKM-Timer/0.1")
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap()
        })
        .await;

    let time = time / 10; // Convert to centiseconds
    let solved_at = chrono::DateTime::from_timestamp_millis(solved_at as i64 * 1000)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let url = format!("{}/result/enter", crate::API_URL.get().unwrap());
    let body = serde_json::json!({
        "value": time,
        "penalty": penalty,
        "solvedAt": solved_at,
        "espId": esp_id,
        "judgeId": judge_id,
        "competitorId": competitor_id,
        "isDelegate": is_delegate,
    });

    let res = client.post(&url).json(&body).send().await.map_err(|e| {
        error!("Error sending solve entry (send error): {}", e);

        ApiErrorRes {
            message: format!("Error sending solve entry"),
            should_reset_time: false,
        }
    })?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();

    trace!("Solve entry response (SC: {status_code}): {}", text);
    if !success {
        error!("Error sending solve entry (not success): {}", text);

        let res = serde_json::from_str::<ApiErrorRes>(&text).map_err(|_| ApiErrorRes {
            message: format!("Error parsing error message"),
            should_reset_time: false,
        })?;

        return Err(res);
    }

    Ok(())
}

// It returns (should_update, use_stable_releases)
pub async fn should_update_devices() -> Result<(bool, bool)> {
    // TODO: make this client as global or sth
    let client = API_CLIENT
        .get_or_init(|| async {
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .user_agent("FKM-Timer/0.1")
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap()
        })
        .await;

    let url = format!(
        "{}/competition/should-update",
        crate::API_URL.get().unwrap()
    );

    let res = client.get(&url).send().await?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();

    trace!("Should update response (SC: {status_code}): {}", text);
    if !success {
        return Err(anyhow::anyhow!("Cannot get 'should-update' status"));
    }

    let json: serde_json::Value = serde_json::from_str(&text)?;
    let should_update = json
        .get("shouldUpdate")
        .ok_or_else(|| anyhow::anyhow!("Field not found"))?
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("Cannot convert to boolean"))?;

    let use_stable_releases = json
        .get("useStableReleases")
        .ok_or_else(|| anyhow::anyhow!("Field not found"))?
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("Cannot convert to boolean"))?;

    Ok((should_update, use_stable_releases))
}
