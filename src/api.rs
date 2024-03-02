use anyhow::Result;
use tokio::sync::OnceCell;

static API_CLIENT: OnceCell<reqwest::Client> = OnceCell::const_new();

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorInfo {
    pub id: i64,
    pub registrant_id: i64,
    pub name: String,
    pub wca_id: String,
    pub country_iso2: String,
    pub gender: String,
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
    let res = client.get(&url).send().await.map_err(|_| ApiErrorRes {
        message: format!("Error getting competitor info"),
        should_reset_time: false,
    })?;

    if !res.status().is_success() {
        println!("Error getting competitor info: {:?}", res);
        return Err(res.json::<ApiErrorRes>().await.map_err(|_| ApiErrorRes {
            message: format!("Error parsing error message"),
            should_reset_time: false,
        })?);
    }

    let info = res
        .json::<CompetitorInfo>()
        .await
        .map_err(|_| ApiErrorRes {
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

    let res = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|_| ApiErrorRes {
            message: format!("Error sending solve entry"),
            should_reset_time: false,
        })?;

    if !res.status().is_success() {
        println!("Error sending solve entry: {:?}", res);
        let res = res.json::<ApiErrorRes>().await.map_err(|_| ApiErrorRes {
            message: format!("Error parsing error message"),
            should_reset_time: false,
        })?;

        return Err(res);
    }

    Ok(())
}

pub async fn should_update_devices() -> Result<bool> {
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
    if !res.status().is_success() {
        return Err(anyhow::anyhow!("Cannot get 'should-update' status"));
    }

    let json: serde_json::Value = res.json().await?;
    let should_update = json
        .get("shouldUpdate")
        .ok_or_else(|| anyhow::anyhow!("Field not found"))?
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("Cannot convert to boolean"))?;

    Ok(should_update)
}
