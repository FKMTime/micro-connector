use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    time::Duration,
};

pub const SERVICE_TYPE: &str = "_fkmtime._tcp.local.";
pub const INSTANCE_NAME: &str = "fkmtime_microconnector";
pub const HOST_NAME: &str = "fkmtime.local.";

/// Matches the adapter path, which re-posts its registration every 30 s.
const INTERFACE_CHECK_INTERVAL: Duration = Duration::from_secs(30);

pub async fn register_mdns(port: &u16) -> Result<()> {
    let adapter_api = crate::adapter::adapter_api_url();
    let client = reqwest::Client::new();
    if let Ok(res) = client.get(&adapter_api).send().await
        && res.status().is_success()
    {
        tokio::task::spawn(crate::adapter::register_mdns(client, adapter_api, *port));
        return Ok(());
    }

    tokio::task::spawn(watch_interfaces(*port));
    Ok(())
}

/// Keeps one mDNS service per local IPv4 address registered, following
/// addresses that appear or disappear after startup (e.g. a LAN bridge that
/// only gets its IP once the router has finished booting).
///
/// Each IP gets its own daemon: a daemon only announces a fixed-address
/// service on interfaces it already knows, and a fresh daemon enumerates the
/// interfaces on creation, so the new address is announced immediately.
async fn watch_interfaces(port: u16) {
    let mut daemons: BTreeMap<IpAddr, ServiceDaemon> = BTreeMap::new();
    let mut interval = tokio::time::interval(INTERFACE_CHECK_INTERVAL);
    loop {
        interval.tick().await;

        let current = match local_ip_address::list_afinet_netifas() {
            Ok(netifas) => netifas.into_iter().map(|(_, ip)| ip).collect::<Vec<_>>(),
            Err(e) => {
                tracing::error!("Cannot list network interfaces: {e:?}");
                continue;
            }
        };

        let registered = daemons.keys().copied().collect();
        let changes = diff_interfaces(&registered, &current);

        for ip in changes.removed {
            if let Some(mdns) = daemons.remove(&ip) {
                tracing::info!("Unregistering mDNS service for {ip}");
                _ = mdns.unregister(&service_fullname(&ip));
                _ = mdns.shutdown();
            }
        }

        for ip in changes.added {
            match register_service(&ip, port) {
                Ok(mdns) => {
                    tracing::info!("Registered mDNS service for {ip}");
                    daemons.insert(ip, mdns);
                }
                Err(e) => tracing::error!("Cannot register mDNS service for {ip}: {e:?}"),
            }
        }
    }
}

fn register_service(ip: &IpAddr, port: u16) -> Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new()?;

    let ip = ip.to_string();
    let properties = if std::env::var("NO_TLS").is_ok() {
        [("ws", format!("ws://{ip}:{port}"))]
    } else {
        [("ws", format!("wss://{ip}:{port}"))]
    };

    let my_service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name(&ip),
        HOST_NAME,
        &ip,
        port,
        &properties[..],
    )?;
    mdns.register(my_service)?;

    Ok(mdns)
}

fn instance_name(ip: &str) -> String {
    format!("{INSTANCE_NAME}/{ip}")
}

fn service_fullname(ip: &IpAddr) -> String {
    format!("{}.{SERVICE_TYPE}", instance_name(&ip.to_string()))
}

fn is_advertised(ip: &IpAddr) -> bool {
    !(ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() || ip.is_ipv6())
}

#[derive(Debug, PartialEq)]
struct InterfaceChanges {
    added: Vec<IpAddr>,
    removed: Vec<IpAddr>,
}

fn diff_interfaces(registered: &BTreeSet<IpAddr>, current: &[IpAddr]) -> InterfaceChanges {
    let current: BTreeSet<IpAddr> = current.iter().copied().filter(is_advertised).collect();

    InterfaceChanges {
        added: current.difference(registered).copied().collect(),
        removed: registered.difference(&current).copied().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn set(ips: &[&str]) -> BTreeSet<IpAddr> {
        ips.iter().map(|s| ip(s)).collect()
    }

    #[test]
    fn adds_ip_that_appeared() {
        let changes = diff_interfaces(&set(&["192.0.2.1"]), &[ip("192.0.2.1"), ip("198.51.100.1")]);

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![ip("198.51.100.1")],
                removed: vec![],
            }
        );
    }

    #[test]
    fn removes_ip_that_disappeared() {
        let changes = diff_interfaces(&set(&["192.0.2.1", "198.51.100.1"]), &[ip("192.0.2.1")]);

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![],
                removed: vec![ip("198.51.100.1")],
            }
        );
    }

    #[test]
    fn no_changes_when_unchanged() {
        let changes = diff_interfaces(
            &set(&["192.0.2.1", "198.51.100.1"]),
            &[ip("198.51.100.1"), ip("192.0.2.1")],
        );

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![],
                removed: vec![],
            }
        );
    }

    #[test]
    fn skips_filtered_addresses() {
        let changes = diff_interfaces(
            &set(&[]),
            &[
                ip("127.0.0.1"),
                ip("224.0.0.251"),
                ip("0.0.0.0"),
                ip("fe80::1"),
                ip("::1"),
                ip("198.51.100.1"),
            ],
        );

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![ip("198.51.100.1")],
                removed: vec![],
            }
        );
    }
}
