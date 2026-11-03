use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::{collections::BTreeMap, net::IpAddr, time::Duration};

pub const SERVICE_TYPE: &str = "_fkmtime._tcp.local.";
pub const INSTANCE_NAME: &str = "fkmtime_microconnector";
pub const HOST_NAME: &str = "fkmtime.local.";

/// Matches mdns-sd's own IP check interval, so a link that comes up is
/// served within seconds.
const INTERFACE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

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

/// A local address as seen on one check.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LocalAddr {
    ip: IpAddr,
    if_index: Option<u32>,
    /// The interface has carrier (IFF_RUNNING), the same test mdns-sd uses
    /// to decide which interfaces it serves.
    up: bool,
}

/// Keeps one mDNS service per local IPv4 address registered, following
/// addresses that appear, disappear, or whose link goes up or down after
/// startup (e.g. a LAN bridge that gets its IP seconds before its link is up
/// while the router boots).
///
/// Each IP gets its own daemon, created only once the address's interface is
/// up. mdns-sd only serves interfaces that are up when the daemon starts:
/// one that comes up later is joined by its IP check, but a service with a
/// fixed address is never announced on it and its queries are ignored. So a
/// daemon created too early stays silent forever; this loop re-creates it
/// instead, and turns off the daemon's own IP check so it cannot drop and
/// re-add an interface into that same silent state on a link flap.
async fn watch_interfaces(port: u16) {
    let mut registered: BTreeMap<IpAddr, (Option<u32>, ServiceDaemon)> = BTreeMap::new();
    let mut interval = tokio::time::interval(INTERFACE_CHECK_INTERVAL);
    loop {
        interval.tick().await;

        let current = match if_addrs::get_if_addrs() {
            Ok(interfaces) => interfaces
                .into_iter()
                .map(|intf| LocalAddr {
                    ip: intf.ip(),
                    if_index: intf.index,
                    up: intf.is_oper_up(),
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::error!("Cannot list network interfaces: {e:?}");
                continue;
            }
        };

        let served = registered
            .iter()
            .map(|(ip, (if_index, _))| (*ip, *if_index))
            .collect();
        let changes = diff_interfaces(&served, &current);

        for ip in changes.removed {
            if let Some((_, mdns)) = registered.remove(&ip) {
                tracing::info!("Unregistering mDNS service for {ip}");
                _ = mdns.unregister(&service_fullname(&ip));
                _ = mdns.shutdown();
            }
        }

        for addr in changes.added {
            match register_service(&addr.ip, port) {
                Ok(mdns) => {
                    tracing::info!("Registered mDNS service for {}", addr.ip);
                    registered.insert(addr.ip, (addr.if_index, mdns));
                }
                Err(e) => tracing::error!("Cannot register mDNS service for {}: {e:?}", addr.ip),
            }
        }
    }
}

fn register_service(ip: &IpAddr, port: u16) -> Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new()?;
    // `watch_interfaces` owns interface changes; see there for why.
    mdns.set_ip_check_interval(0)?;

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
    added: Vec<LocalAddr>,
    removed: Vec<IpAddr>,
}

/// Compares the served addresses (IP and interface index) with the current
/// ones. Only addresses on an up interface are served; one whose interface
/// went down, was replaced (new index) or is gone is removed, and an address
/// is added once its interface is up. A replaced interface shows up in both
/// lists, so its daemon is re-created.
fn diff_interfaces(
    served: &BTreeMap<IpAddr, Option<u32>>,
    current: &[LocalAddr],
) -> InterfaceChanges {
    let mut wanted: BTreeMap<IpAddr, LocalAddr> = BTreeMap::new();
    for addr in current.iter().filter(|a| a.up && is_advertised(&a.ip)) {
        wanted.entry(addr.ip).or_insert(*addr);
    }

    let removed: Vec<IpAddr> = served
        .iter()
        .filter(|(ip, if_index)| wanted.get(ip).is_none_or(|a| a.if_index != **if_index))
        .map(|(ip, _)| *ip)
        .collect();
    let added = wanted
        .into_values()
        .filter(|a| !served.contains_key(&a.ip) || removed.contains(&a.ip))
        .collect();

    InterfaceChanges { added, removed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn up(s: &str, if_index: u32) -> LocalAddr {
        LocalAddr {
            ip: ip(s),
            if_index: Some(if_index),
            up: true,
        }
    }

    fn down(s: &str, if_index: u32) -> LocalAddr {
        LocalAddr {
            up: false,
            ..up(s, if_index)
        }
    }

    fn served(addrs: &[(&str, u32)]) -> BTreeMap<IpAddr, Option<u32>> {
        addrs.iter().map(|(s, i)| (ip(s), Some(*i))).collect()
    }

    #[test]
    fn adds_address_that_is_up_at_start() {
        let changes = diff_interfaces(&served(&[]), &[up("192.0.2.1", 3)]);

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![up("192.0.2.1", 3)],
                removed: vec![],
            }
        );
    }

    #[test]
    fn waits_for_link_then_adds_address() {
        let before_link = diff_interfaces(&served(&[]), &[down("192.0.2.1", 3)]);
        assert_eq!(
            before_link,
            InterfaceChanges {
                added: vec![],
                removed: vec![],
            }
        );

        let after_link = diff_interfaces(&served(&[]), &[up("192.0.2.1", 3)]);
        assert_eq!(
            after_link,
            InterfaceChanges {
                added: vec![up("192.0.2.1", 3)],
                removed: vec![],
            }
        );
    }

    #[test]
    fn removes_address_whose_link_went_down() {
        let changes = diff_interfaces(&served(&[("192.0.2.1", 3)]), &[down("192.0.2.1", 3)]);

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![],
                removed: vec![ip("192.0.2.1")],
            }
        );
    }

    #[test]
    fn recreates_address_moved_to_new_interface() {
        let changes = diff_interfaces(&served(&[("192.0.2.1", 3)]), &[up("192.0.2.1", 7)]);

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![up("192.0.2.1", 7)],
                removed: vec![ip("192.0.2.1")],
            }
        );
    }

    #[test]
    fn adds_address_that_appeared() {
        let changes = diff_interfaces(
            &served(&[("192.0.2.1", 3)]),
            &[up("192.0.2.1", 3), up("198.51.100.1", 4)],
        );

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![up("198.51.100.1", 4)],
                removed: vec![],
            }
        );
    }

    #[test]
    fn removes_address_that_disappeared() {
        let changes = diff_interfaces(
            &served(&[("192.0.2.1", 3), ("198.51.100.1", 4)]),
            &[up("192.0.2.1", 3)],
        );

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
            &served(&[("192.0.2.1", 3), ("198.51.100.1", 4)]),
            &[up("198.51.100.1", 4), up("192.0.2.1", 3)],
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
            &served(&[]),
            &[
                up("127.0.0.1", 1),
                up("224.0.0.251", 2),
                up("0.0.0.0", 2),
                up("fe80::1", 2),
                up("::1", 1),
                up("198.51.100.1", 2),
            ],
        );

        assert_eq!(
            changes,
            InterfaceChanges {
                added: vec![up("198.51.100.1", 2)],
                removed: vec![],
            }
        );
    }
}
