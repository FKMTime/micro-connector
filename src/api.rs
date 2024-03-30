use anyhow::{anyhow, Result};
use tokio::sync::OnceCell;
use tracing::{error, trace};

use crate::structs::{self, CompetitionStatusResp};

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorInfo {
    pub id: String,
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

pub async fn get_competitor_info(
    client: &reqwest::Client,
    api_url: &str,
    card_id: u128,
) -> Result<CompetitorInfo, ApiErrorRes> {
    let url = format!("{api_url}/person/card/{card_id}");
    let res = client
        .get(&url)
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .send()
        .await
        .map_err(|e| {
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

pub async fn mark_attendance(
    client: &reqwest::Client,
    api_url: &str,
    esp_id: u32,
    card_id: u128,
) -> Result<()> {
    let url = format!("{api_url}/attendance");
    let body = serde_json::json!({
        "espId": esp_id,
        "cardId": card_id,
    });
    let res = client
        .post(&url)
        .body(body.to_string())
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .header("Content-Type", "application/json")
        .send()
        .await?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    trace!("Attendance mark response (SC: {status_code}): {}", text);

    if !success {
        error!("Error marking attendance (not success): {}", text);
        return Err(anyhow!("Error marking attendance"));
    }

    Ok(())
}

pub async fn send_solve_entry(
    client: &reqwest::Client,
    api_url: &str,
    time: u128,
    penalty: i64,
    solved_at: u128,
    esp_id: u32,
    judge_id: u128,
    competitor_id: u128,
    is_delegate: bool,
    session_id: &str,
    inspection_time: u128,
) -> Result<(), ApiErrorRes> {
    let time = time / 10; // Convert to centiseconds
    let solved_at = chrono::DateTime::from_timestamp_millis(solved_at as i64 * 1000)
        .ok_or_else(|| ApiErrorRes {
            message: format!("Error parsing timestamp"),
            should_reset_time: false,
        })?
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let url = format!("{api_url}/result/enter");
    let body = serde_json::json!({
        "value": time,
        "penalty": penalty,
        "solvedAt": solved_at,
        "espId": esp_id,
        "judgeId": judge_id,
        "competitorId": competitor_id,
        "isDelegate": is_delegate,
        "sessionId": session_id,
        "inspectionTime": inspection_time,
    });

    let res = client
        .post(&url)
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| {
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

pub async fn send_battery_status(
    client: &reqwest::Client,
    api_url: &str,
    esp_id: u32,
    battery: f64,
) -> Result<()> {
    let battery: u8 = battery.round() as u8;

    let url = format!("{api_url}/device/battery");
    let body = serde_json::json!({
        "espId": esp_id,
        "batteryPercentage": battery,
    });

    let res = client
        .post(&url)
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .json(&body)
        .send()
        .await?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();

    trace!("Battery status response (SC: {status_code}): {}", text);
    if !success {
        anyhow::bail!("Error sending battery status (not success): {}", text);
    }

    Ok(())
}

pub async fn get_competition_status(
    client: &reqwest::Client,
    api_url: &str,
) -> Result<CompetitionStatusResp> {
    let url = format!("{api_url}/competition/status");

    let res = client
        .get(&url)
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .send()
        .await?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();

    trace!("Competition status response (SC: {status_code}): {}", text);
    if !success {
        return Err(anyhow::anyhow!("Cannot get competition status"));
    }

    let json: CompetitionStatusResp = serde_json::from_str(&text)?;
    Ok(json)
}

pub async fn add_device(
    client: &reqwest::Client,
    api_url: &str,
    esp_id: u32,
    firmware_type: &str,
) -> Result<()> {
    let url = format!("{api_url}/device/connect");
    let body = serde_json::json!({
        "espId": esp_id,
        "type": firmware_type,
    });

    let res = client
        .post(&url)
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .json(&body)
        .send()
        .await?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();

    trace!("Add device response (SC: {status_code}): {}", text);
    if !success {
        anyhow::bail!("Error adding device (not success): {}", text);
    }

    Ok(())
}

pub async fn get_wifi_settings(
    client: &reqwest::Client,
    api_url: &str,
) -> Result<(String, String)> {
    let url = format!("{api_url}/competition/wifi");
    let res = client
        .get(&url)
        .header(
            "Authorization",
            crate::api::ApiClient::get_fkm_api_token().expect("API_TOKEN not set"),
        )
        .send()
        .await?;

    let success = res.status().is_success();
    let status_code = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();

    trace!("Wifi settings response (SC: {status_code}): {}", text);
    if !success {
        anyhow::bail!("Error getting wifi settings (not success): {}", text);
    }

    let json: structs::WifiSettings = serde_json::from_str(&text)?;
    Ok((json.wifi_ssid, json.wifi_password))
}

static API_URL: OnceCell<String> = OnceCell::const_new();
static API_CLIENT: OnceCell<reqwest::Client> = OnceCell::const_new();
static FKM_API_TOKEN: OnceCell<String> = OnceCell::const_new();
pub struct ApiClient {}
impl ApiClient {
    pub fn set_api_client(
        client: reqwest::Client,
        api_url: String,
        fkm_api_token: String,
    ) -> Result<()> {
        API_CLIENT.set(client)?;
        API_URL.set(api_url)?;
        FKM_API_TOKEN.set(fkm_api_token)?;

        Ok(())
    }

    pub fn get_api_client() -> Result<(reqwest::Client, String)> {
        let client = API_CLIENT
            .get()
            .ok_or_else(|| anyhow!("API_CLIENT not set"))?
            .to_owned();

        let api_url = API_URL
            .get()
            .ok_or_else(|| anyhow!("API_URL not set"))?
            .to_owned();

        Ok((client, api_url))
    }

    pub fn get_fkm_api_token() -> Result<String> {
        let api_token = FKM_API_TOKEN
            .get()
            .ok_or_else(|| anyhow!("API_TOKEN not set"))?;

        Ok(format!("Token {api_token}"))
    }
}
