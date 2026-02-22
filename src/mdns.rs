use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_fkmtime._tcp.local.";
pub const INSTANCE_NAME: &str = "fkmtime_microconnector";
pub const HOST_NAME: &str = "fkmtime.local.";

pub async fn register_mdns(port: &u16) -> Result<()> {
    let adapter_api = crate::adapter::adapter_api_url();
    let client = reqwest::Client::new();
    if let Ok(res) = client.get(&adapter_api).send().await
        && res.status().is_success()
    {
        tokio::task::spawn(crate::adapter::register_mdns(client, adapter_api, *port));
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
            &format!("{INSTANCE_NAME}/{ip}"),
            HOST_NAME,
            &ip,
            *port,
            &properties[..],
        )?;
        mdns.register(my_service)?;
    }

    Ok(())
}
