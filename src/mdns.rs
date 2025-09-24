use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub fn register_mdns(port: &u16) -> Result<()> {
    let network_interfaces = local_ip_address::list_afinet_netifas().expect("afinet list failed");

    for (_, ip) in network_interfaces.iter() {
        if ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() || ip.is_ipv6() {
            continue;
        }

        if ip.to_string().starts_with("172.") {
            continue;
        }

        let mdns = ServiceDaemon::new()?;

        let service_type = "_stackmat._tcp.local.";
        let instance_name = "stackmat_backend";
        let ip = ip.to_string();
        let host_name = "stackmat.local.";
        let properties = if std::env::var("TLS").is_ok() {
            [("ws", format!("wss://{ip}:{port}"))]
        } else {
            [("ws", format!("ws://{ip}:{port}"))]
        };

        let my_service = ServiceInfo::new(
            service_type,
            instance_name,
            &host_name,
            ip,
            *port,
            &properties[..],
        )?;
        mdns.register(my_service)?;
    }

    Ok(())
}
