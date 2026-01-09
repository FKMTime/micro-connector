use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use serde::Serialize;
use std::{path::Path, time::Duration};

const SERVICE_TYPE: &str = "_fkmtime._tcp.local.";
const INSTANCE_NAME: &str = "fkmtime_microconnector";
const HOST_NAME: &str = "fkmtime.local.";

pub async fn register_mdns(port: &u16) -> Result<()> {
    let mdns_api = std::env::var("MDNS_ADAPTER_API").unwrap_or_else(|_| {
        if is_running_in_docker() {
            "http://host.docker.internal:3127"
        } else {
            "http://localhost:3127"
        }
        .to_string()
    });

    let client = reqwest::Client::new();
    if let Ok(res) = client.get(&mdns_api).send().await
        && res.status().is_success()
    {
        tokio::task::spawn(mdns_adapter_api(client, mdns_api, *port));
        return Ok(());
    }

    let network_interfaces = local_ip_address::list_afinet_netifas().expect("afinet list failed");
    for (_, ip) in network_interfaces.iter() {
        if ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() || ip.is_ipv6() {
            continue;
        }

        let mdns = ServiceDaemon::new()?;

        let ip = ip.to_string();
        let properties = if std::env::var("NO_TLS").is_ok() {
            [("ws", format!("ws://{ip}:{port}"))]
        } else {
            [("ws", format!("wss://{ip}:{port}"))]
        };

        let my_service = ServiceInfo::new(
            SERVICE_TYPE,
            INSTANCE_NAME,
            HOST_NAME,
            &ip,
            *port,
            &properties[..],
        )?;
        mdns.register(my_service)?;

        // backwards compatible with old fws
        let my_service = ServiceInfo::new(
            &SERVICE_TYPE.replace("fkmtime", "stackmat"),
            &INSTANCE_NAME.replace("fkmtime", "stackmat"),
            &HOST_NAME.replace("fkmtime", "stackmat"),
            ip,
            *port,
            &properties[..],
        )?;
        mdns.register(my_service)?;
    }

    Ok(())
}

fn is_running_in_docker() -> bool {
    if Path::new("/.dockerenv").exists() {
        return true;
    }

    if let Ok(contents) = std::fs::read_to_string("/proc/self/cgroup")
        && contents.contains("docker")
    {
        return true;
    }

    false
}

#[derive(Debug, Serialize)]
pub struct RegisterMdnsApi {
    pub all_interfaces: bool,
    pub properties: Vec<(String, String)>,
    pub service_type: String,
    pub instance_name: String,
    pub ip: Option<String>,
    pub port: u16,
    pub host_name: String,
}
async fn mdns_adapter_api(client: reqwest::Client, api_url: String, port: u16) -> Result<()> {
    tracing::info!("Using MDNS Adapter api!");
    let mut data = RegisterMdnsApi {
        all_interfaces: true,
        properties: Vec::new(),
        service_type: SERVICE_TYPE.to_string(),
        instance_name: INSTANCE_NAME.to_string(),
        ip: None,
        port,
        host_name: HOST_NAME.to_string(),
    };
    if std::env::var("NO_TLS").is_ok() {
        data.properties
            .push(("ws".to_string(), format!("ws://{{IF_IP}}:{port}")));
    } else {
        data.properties
            .push(("ws".to_string(), format!("wss://{{IF_IP}}:{port}")));
    }

    let data_json = serde_json::to_string(&data)?;
    tracing::debug!("Data json: {data_json}");
    loop {
        _ = client
            .post(&api_url)
            .body(data_json.clone())
            .header("Content-Type", "application/json")
            .send()
            .await;

        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
