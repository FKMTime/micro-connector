//! The access point's view of each station (SPEC-008), when the connector runs on the router
//! that is also the Wi-Fi access point: signal, tx bitrate and retries from an nl80211 station
//! dump, matched to stations through their websocket address and the host's neighbour table.
//!
//! Everywhere else (Docker on macOS, a separate access point, no nl80211 access) there is no AP
//! interface or no matching station, and nothing is sent: that is the normal case for some
//! competitions, not an error.

use crate::structs::SharedAppState;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

mod nl80211;

const READ_INTERVAL: Duration = Duration::from_secs(10);
const NEIGHBOUR_TABLE: &str = "/proc/net/arp";
/// `ATF_COM` in /proc/net/arp: the entry has a resolved MAC
const ARP_FLAG_COMPLETE: u32 = 0x2;

pub type Mac = [u8; 6];

/// One station from an nl80211 station dump, the fields `iw dev <if> station dump` shows
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StationDump {
    pub mac: Mac,
    /// dBm, last received frame
    pub signal: Option<i8>,
    /// dBm, averaged by the driver
    pub signal_avg: Option<i8>,
    pub tx_bitrate_kbps: Option<u32>,
    pub tx_packets: Option<u32>,
    pub tx_retries: Option<u32>,
}

/// What `StationLink` carries for one station
#[derive(Debug, Clone, PartialEq)]
pub struct StationLink {
    pub esp_id: u32,
    pub ap_signal: Option<i8>,
    pub tx_bitrate_kbps: Option<u32>,
    pub tx_retries_percent: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TxCounters {
    packets: u32,
    retries: u32,
}

/// The tx counters from the previous dump, per station MAC, so retries reflect the last
/// interval and not the whole day.
#[derive(Debug, Default)]
pub struct RetryTracker(HashMap<Mac, TxCounters>);

impl RetryTracker {
    /// Retries per 100 packets sent since the previous dump, per MAC in this one. Stations that
    /// left the dump are forgotten, so one that comes back starts over.
    pub fn update(&mut self, dump: &[StationDump]) -> HashMap<Mac, Option<u32>> {
        let mut previous = std::mem::take(&mut self.0);

        dump.iter()
            .map(|station| {
                let now = station
                    .tx_packets
                    .zip(station.tx_retries)
                    .map(|(packets, retries)| TxCounters { packets, retries });
                let before = previous.remove(&station.mac);
                if let Some(now) = now {
                    self.0.insert(station.mac, now);
                }

                (
                    station.mac,
                    now.and_then(|now| retries_percent(before?, now)),
                )
            })
            .collect()
    }
}

/// `None` when nothing was sent since `before`, or a counter went backwards (the station
/// reassociated, or the driver reset its counters).
fn retries_percent(before: TxCounters, now: TxCounters) -> Option<u32> {
    let packets = now.packets.checked_sub(before.packets)?;
    let retries = now.retries.checked_sub(before.retries)?;
    if packets == 0 {
        return None;
    }

    Some((f64::from(retries) * 100.0 / f64::from(packets)).round() as u32)
}

/// A driver without a reading reports 0
fn signal_dbm(station: &StationDump) -> Option<i8> {
    [station.signal_avg, station.signal]
        .into_iter()
        .flatten()
        .find(|&dbm| dbm != 0)
}

pub fn parse_mac(text: &str) -> Option<Mac> {
    let mut mac = [0; 6];
    let mut parts = text.split(':');
    for byte in &mut mac {
        *byte = u8::from_str_radix(parts.next()?, 16).ok()?;
    }

    parts.next().is_none().then_some(mac)
}

/// IPv4 → MAC from `/proc/net/arp`, resolved entries only
pub fn parse_neighbours(table: &str) -> HashMap<Ipv4Addr, Mac> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [ip, _hw_type, flags, mac, ..] = fields[..] else {
                return None;
            };
            let flags = u32::from_str_radix(flags.trim_start_matches("0x"), 16).ok()?;
            let mac = parse_mac(mac).filter(|&mac| mac != [0; 6])?;

            (flags & ARP_FLAG_COMPLETE != 0).then_some((ip.parse().ok()?, mac))
        })
        .collect()
}

/// One `StationLink` per connected station found among the access point's clients
pub fn station_links(
    peers: &[(u32, IpAddr)],
    neighbours: &HashMap<Ipv4Addr, Mac>,
    dump: &[StationDump],
    retries: &HashMap<Mac, Option<u32>>,
) -> Vec<StationLink> {
    let mut links: Vec<StationLink> = peers
        .iter()
        .filter_map(|&(esp_id, ip)| {
            let IpAddr::V4(ip) = ip.to_canonical() else {
                return None;
            };
            let mac = neighbours.get(&ip)?;
            let station = dump.iter().find(|station| station.mac == *mac)?;

            Some(StationLink {
                esp_id,
                ap_signal: signal_dbm(station),
                tx_bitrate_kbps: station.tx_bitrate_kbps,
                tx_retries_percent: retries.get(mac).copied().flatten(),
            })
        })
        .collect();

    links.sort_by_key(|link| link.esp_id);
    links
}

/// Why a round sent what it did; logged when it changes, not every 10 s
#[derive(Debug)]
enum Outcome {
    Unavailable(String),
    NoApInterface,
    NoMatch,
    Sending,
}

impl Outcome {
    fn same_kind(&self, other: &Outcome) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    fn log(&self) {
        match self {
            Outcome::Unavailable(e) => {
                tracing::info!("Station link: not reading the access point (nl80211: {e})")
            }
            Outcome::NoApInterface => {
                tracing::info!("Station link: no Wi-Fi interface in AP mode on this machine")
            }
            Outcome::NoMatch => {
                tracing::info!("Station link: no connected station is a client of this AP")
            }
            Outcome::Sending => {
                tracing::info!("Station link: sending the access point's view of stations")
            }
        }
    }
}

pub fn spawn(state: SharedAppState) {
    tokio::task::spawn(async move {
        let mut reader = nl80211::Reader::new();
        let mut tracker = RetryTracker::default();
        let mut last: Option<Outcome> = None;

        let mut interval = tokio::time::interval(READ_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;

            let peers = known_peers(&state).await;
            if peers.is_empty() {
                continue;
            }

            let (outcome, links) = read_links(&mut reader, &mut tracker, &peers).await;
            if !last.as_ref().is_some_and(|last| last.same_kind(&outcome)) {
                outcome.log();
            }
            last = Some(outcome);

            for link in links {
                _ = crate::socket::api::send_station_link(link).await;
            }
        }
    });
}

/// Connected stations the backend knows, with their address
async fn known_peers(state: &SharedAppState) -> Vec<(u32, IpAddr)> {
    let mut peers = state.connected_devices.peers();
    let inner = state.inner.read().await;
    peers.retain(|(esp_id, _)| inner.devices_settings.contains_key(esp_id));
    peers
}

async fn read_links(
    reader: &mut nl80211::Reader,
    tracker: &mut RetryTracker,
    peers: &[(u32, IpAddr)],
) -> (Outcome, Vec<StationLink>) {
    let dump = match reader.ap_station_dump().await {
        Ok(Some(dump)) => dump,
        Ok(None) => return (Outcome::NoApInterface, Vec::new()),
        Err(e) => return (Outcome::Unavailable(format!("{e:#}")), Vec::new()),
    };
    let retries = tracker.update(&dump);

    let neighbours = tokio::fs::read_to_string(NEIGHBOUR_TABLE)
        .await
        .map(|table| parse_neighbours(&table))
        .unwrap_or_default();

    let links = station_links(peers, &neighbours, &dump, &retries);
    let outcome = if links.is_empty() {
        Outcome::NoMatch
    } else {
        Outcome::Sending
    };
    (outcome, links)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATION_A: Mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x0a];
    const STATION_B: Mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x0b];
    const LAPTOP: Mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x0c];
    const ESP_A: u32 = 0x1111_AAAA;
    const ESP_B: u32 = 0x2222_BBBB;

    /// Made-up addresses: a resolved entry per device, an incomplete one, and a wired client
    const ARP: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
10.0.0.21        0x1         0x2         02:00:00:00:00:0a     *        br-lan
10.0.0.22        0x1         0x2         02:00:00:00:00:0b     *        br-lan
10.0.0.23        0x1         0x0         00:00:00:00:00:00     *        br-lan
10.0.0.24        0x1         0x2         02:00:00:00:00:0c     *        br-lan
";

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    fn station(mac: Mac, tx_packets: u32, tx_retries: u32) -> StationDump {
        StationDump {
            mac,
            signal: Some(-70),
            signal_avg: Some(-68),
            tx_bitrate_kbps: Some(6_000),
            tx_packets: Some(tx_packets),
            tx_retries: Some(tx_retries),
        }
    }

    #[test]
    fn parses_mac() {
        assert_eq!(parse_mac("02:00:00:00:00:0a"), Some(STATION_A));
        assert_eq!(parse_mac("02:00:00:00:00:0A"), Some(STATION_A));
        assert_eq!(parse_mac("02:00:00:00:00"), None);
        assert_eq!(parse_mac("02:00:00:00:00:0a:ff"), None);
        assert_eq!(parse_mac("02:00:00:00:00:zz"), None);
    }

    #[test]
    fn neighbours_keep_resolved_entries_only() {
        let neighbours = parse_neighbours(ARP);

        assert_eq!(neighbours.len(), 3);
        assert_eq!(neighbours[&Ipv4Addr::new(10, 0, 0, 21)], STATION_A);
        assert_eq!(neighbours[&Ipv4Addr::new(10, 0, 0, 22)], STATION_B);
        assert!(!neighbours.contains_key(&Ipv4Addr::new(10, 0, 0, 23)));
    }

    #[test]
    fn neighbours_of_an_empty_or_odd_table() {
        assert!(parse_neighbours("").is_empty());
        assert!(parse_neighbours("IP address HW type Flags HW address Mask Device\n").is_empty());
        assert!(parse_neighbours("header\nnot an arp line\n").is_empty());
    }

    #[test]
    fn first_reading_has_no_retries() {
        let mut tracker = RetryTracker::default();

        let retries = tracker.update(&[station(STATION_A, 1_000, 400)]);
        assert_eq!(retries[&STATION_A], None);
    }

    #[test]
    fn retries_over_the_last_interval() {
        let mut tracker = RetryTracker::default();
        tracker.update(&[station(STATION_A, 1_000, 400)]);

        // 50 packets, 60 retries since: more than one retry per packet is possible
        let retries = tracker.update(&[station(STATION_A, 1_050, 460)]);
        assert_eq!(retries[&STATION_A], Some(120));

        let retries = tracker.update(&[station(STATION_A, 1_250, 470)]);
        assert_eq!(retries[&STATION_A], Some(5));
    }

    #[test]
    fn retries_round_to_a_whole_percent() {
        let mut tracker = RetryTracker::default();
        tracker.update(&[station(STATION_A, 0, 0)]);

        let retries = tracker.update(&[station(STATION_A, 3, 2)]);
        assert_eq!(retries[&STATION_A], Some(67));
    }

    #[test]
    fn no_packets_sent_means_no_retries() {
        let mut tracker = RetryTracker::default();
        tracker.update(&[station(STATION_A, 1_000, 400)]);

        let retries = tracker.update(&[station(STATION_A, 1_000, 400)]);
        assert_eq!(retries[&STATION_A], None);
    }

    #[test]
    fn counter_reset_starts_over() {
        let mut tracker = RetryTracker::default();
        tracker.update(&[station(STATION_A, 9_000, 4_000)]);

        let retries = tracker.update(&[station(STATION_A, 100, 30)]);
        assert_eq!(retries[&STATION_A], None);

        let retries = tracker.update(&[station(STATION_A, 200, 40)]);
        assert_eq!(retries[&STATION_A], Some(10));
    }

    #[test]
    fn retries_counter_reset_alone_starts_over() {
        let mut tracker = RetryTracker::default();
        tracker.update(&[station(STATION_A, 1_000, 400)]);

        let retries = tracker.update(&[station(STATION_A, 1_100, 10)]);
        assert_eq!(retries[&STATION_A], None);
    }

    #[test]
    fn station_that_left_the_dump_starts_over() {
        let mut tracker = RetryTracker::default();
        tracker.update(&[station(STATION_A, 1_000, 400), station(STATION_B, 10, 0)]);
        tracker.update(&[station(STATION_B, 20, 0)]);

        let retries = tracker.update(&[station(STATION_A, 1_100, 410), station(STATION_B, 30, 5)]);
        assert_eq!(retries[&STATION_A], None);
        assert_eq!(retries[&STATION_B], Some(50));
    }

    #[test]
    fn missing_counters_give_no_retries() {
        let mut tracker = RetryTracker::default();
        let no_counters = StationDump {
            mac: STATION_A,
            ..Default::default()
        };
        tracker.update(std::slice::from_ref(&no_counters));

        let retries = tracker.update(&[no_counters]);
        assert_eq!(retries[&STATION_A], None);
    }

    #[test]
    fn links_for_matched_stations_only() {
        let neighbours = parse_neighbours(ARP);
        let dump = [
            station(STATION_A, 1_050, 460),
            StationDump {
                mac: STATION_B,
                signal: Some(-75),
                signal_avg: None,
                tx_bitrate_kbps: Some(1_000),
                tx_packets: None,
                tx_retries: None,
            },
            station(LAPTOP, 5, 0),
        ];
        let retries = HashMap::from([(STATION_A, Some(120)), (STATION_B, None), (LAPTOP, None)]);
        // the last one is connected but its address isn't in the neighbour table
        let peers = [(ESP_B, ip(22)), (ESP_A, ip(21)), (0x3333_CCCC, ip(99))];

        assert_eq!(
            station_links(&peers, &neighbours, &dump, &retries),
            [
                StationLink {
                    esp_id: ESP_A,
                    ap_signal: Some(-68),
                    tx_bitrate_kbps: Some(6_000),
                    tx_retries_percent: Some(120),
                },
                StationLink {
                    esp_id: ESP_B,
                    ap_signal: Some(-75),
                    tx_bitrate_kbps: Some(1_000),
                    tx_retries_percent: None,
                },
            ]
        );
    }

    /// e.g. a station on another access point: in the neighbour table, not in this AP's dump
    #[test]
    fn no_links_when_nothing_matches() {
        let neighbours = parse_neighbours(ARP);
        let dump = [station(LAPTOP, 5, 0)];
        let peers = [(ESP_A, ip(21)), (ESP_B, ip(22))];

        assert!(station_links(&peers, &neighbours, &dump, &HashMap::new()).is_empty());
        assert!(station_links(&peers, &HashMap::new(), &dump, &HashMap::new()).is_empty());
        assert!(station_links(&peers, &neighbours, &[], &HashMap::new()).is_empty());
    }

    #[test]
    fn ipv4_mapped_peer_matches() {
        let neighbours = parse_neighbours(ARP);
        let dump = [station(STATION_A, 1, 0)];
        let mapped = IpAddr::V6(Ipv4Addr::new(10, 0, 0, 21).to_ipv6_mapped());

        let links = station_links(&[(ESP_A, mapped)], &neighbours, &dump, &HashMap::new());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].esp_id, ESP_A);
    }

    #[test]
    fn signal_prefers_the_average_and_skips_zero() {
        let mut dump = station(STATION_A, 1, 0);
        assert_eq!(signal_dbm(&dump), Some(-68));

        dump.signal_avg = Some(0);
        assert_eq!(signal_dbm(&dump), Some(-70));

        dump.signal_avg = None;
        dump.signal = Some(0);
        assert_eq!(signal_dbm(&dump), None);
    }

    #[test]
    fn outcome_logs_once_per_kind() {
        let unavailable = Outcome::Unavailable("permission denied".to_string());
        let unavailable_again = Outcome::Unavailable("other text".to_string());

        assert!(unavailable.same_kind(&unavailable_again));
        assert!(!unavailable.same_kind(&Outcome::NoApInterface));
        assert!(!Outcome::NoMatch.same_kind(&Outcome::Sending));
    }
}
