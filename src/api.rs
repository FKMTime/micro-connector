use anyhow::Result;
use tokio::sync::OnceCell;

static API_CLIENT: OnceCell<reqwest::Client> = OnceCell::const_new();

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SolverInfo {
    pub id: i64,
    pub registrant_id: i64,
    pub name: String,
    pub wca_id: String,
    pub country_iso2: String,
    pub gender: String,
}
pub async fn get_solver_info(card_id: u128) -> Result<SolverInfo> {
    let client = API_CLIENT
        .get_or_init(|| async {
            reqwest::Client::builder()
                .user_agent("FKM-Timer/0.1")
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap()
        })
        .await;

    let url = format!("{}/person/card/{}", crate::API_URL.get().unwrap(), card_id);
    let res = client.get(&url).send().await?;

    if !res.status().is_success() {
        println!("Error getting solver info: {:?}", res);
        return Err(anyhow::anyhow!("Error getting solver info"));
    }

    let info = res.json::<SolverInfo>().await;
    Ok(info?)
}

pub async fn send_solve_entry(
    time: u128,
    penalty: i64,
    solved_at: u128,
    esp_id: u128,
    judge_id: u128,
    competitor_id: u128,
    is_delegate: bool,
) -> Result<()> {
    let client = API_CLIENT
        .get_or_init(|| async {
            reqwest::Client::builder()
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

    let res = client.post(&url).json(&body).send().await?;
    if !res.status().is_success() {
        println!("Error sending solve entry: {:?}", res);
        return Err(anyhow::anyhow!("Error sending solve entry"));
    }

    Ok(())
}
