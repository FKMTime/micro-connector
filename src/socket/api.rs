use crate::socket::structs;

#[derive(Debug)]
pub struct CompetitorInfo {
    pub id: String,
    pub registrant_id: Option<i64>,
    pub name: String,
    pub wca_id: Option<String>,
    pub country_iso2: Option<String>,
    pub gender: String,
    pub can_compete: bool,
}

#[derive(Debug)]
pub struct ApiErrorRes {
    pub message: String,
    pub should_reset_time: bool,
}

pub async fn get_competitor_info(card_id: u128) -> Result<CompetitorInfo, ApiErrorRes> {
    let res = crate::UNIX_SOCKET
        .send_tagged_request(structs::UnixRequestData::PersonInfo { card_id })
        .await;

    todo!()
}
