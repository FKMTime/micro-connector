use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub fn register_mdns(port: &u16) -> Result<()> {
    let local_ip = local_ip_address::local_ip()?;
    let mdns = ServiceDaemon::new()?;

    let service_type = "_stackmat._tcp.local.";
    let instance_name = "stackmat_backend";
    let ip = local_ip.to_string();
    let host_name = format!("ws://{ip}:{port}");
    let properties = [("much", "wow")];

    let my_service = ServiceInfo::new(
        service_type,
        instance_name,
        &host_name,
        ip,
        *port,
        &properties[..],
    )?;
    mdns.register(my_service)?;

    Ok(())
}
