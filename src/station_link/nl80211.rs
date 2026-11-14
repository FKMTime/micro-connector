//! nl80211 station dump for every interface in AP mode. Only Linux has nl80211; elsewhere the
//! reader reports it unavailable and the connector sends nothing.

use super::StationDump;
use anyhow::Result;

#[cfg(target_os = "linux")]
pub use linux::Reader;

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub struct Reader;

#[cfg(not(target_os = "linux"))]
impl Reader {
    pub fn new() -> Self {
        Reader
    }

    /// `Ok(None)` when no interface is in AP mode
    pub async fn ap_station_dump(&mut self) -> Result<Option<Vec<StationDump>>> {
        anyhow::bail!("not available on this OS")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Result, StationDump};
    use futures_util::TryStreamExt;
    use wl_nl80211::{
        Nl80211Attr, Nl80211Handle, Nl80211InterfaceType, Nl80211RateInfo, Nl80211StationInfo,
    };

    /// Keeps one nl80211 connection, and opens a new one after an error.
    #[derive(Default)]
    pub struct Reader {
        handle: Option<Nl80211Handle>,
    }

    impl Reader {
        pub fn new() -> Self {
            Self::default()
        }

        /// `Ok(None)` when no interface is in AP mode
        pub async fn ap_station_dump(&mut self) -> Result<Option<Vec<StationDump>>> {
            let handle = match &self.handle {
                Some(handle) => handle.clone(),
                None => {
                    let (connection, handle, _) = wl_nl80211::new_connection()?;
                    tokio::spawn(connection);
                    self.handle.insert(handle).clone()
                }
            };

            let res = dump(handle).await;
            if res.is_err() {
                self.handle = None;
            }
            res
        }
    }

    async fn dump(handle: Nl80211Handle) -> Result<Option<Vec<StationDump>>> {
        let interfaces: Vec<_> = handle
            .interface()
            .get(Vec::new())
            .execute()
            .await
            .try_collect()
            .await?;
        let ap_interfaces: Vec<u32> = interfaces
            .iter()
            .filter_map(|msg| ap_if_index(&msg.payload.attributes))
            .collect();
        if ap_interfaces.is_empty() {
            return Ok(None);
        }

        let mut stations = Vec::new();
        for if_index in ap_interfaces {
            let msgs: Vec<_> = handle
                .station()
                .dump(if_index)
                .execute()
                .await
                .try_collect()
                .await?;
            stations.extend(
                msgs.iter()
                    .filter_map(|msg| station_dump(&msg.payload.attributes)),
            );
        }

        Ok(Some(stations))
    }

    fn ap_if_index(attrs: &[Nl80211Attr]) -> Option<u32> {
        let is_ap = attrs
            .iter()
            .any(|attr| matches!(attr, Nl80211Attr::IfType(Nl80211InterfaceType::Ap)));

        attrs
            .iter()
            .find_map(|attr| match attr {
                Nl80211Attr::IfIndex(index) => Some(*index),
                _ => None,
            })
            .filter(|_| is_ap)
    }

    fn station_dump(attrs: &[Nl80211Attr]) -> Option<StationDump> {
        let mut station = StationDump {
            mac: attrs.iter().find_map(|attr| match attr {
                Nl80211Attr::Mac(mac) => Some(*mac),
                _ => None,
            })?,
            ..Default::default()
        };

        let info = attrs.iter().find_map(|attr| match attr {
            Nl80211Attr::StationInfo(info) => Some(info),
            _ => None,
        })?;
        for item in info {
            match item {
                Nl80211StationInfo::Signal(dbm) => station.signal = Some(*dbm),
                Nl80211StationInfo::SignalAvg(dbm) => station.signal_avg = Some(*dbm),
                Nl80211StationInfo::TxBitrate(rate) => station.tx_bitrate_kbps = bitrate_kbps(rate),
                Nl80211StationInfo::TxPackets(packets) => station.tx_packets = Some(*packets),
                Nl80211StationInfo::TxRetries(retries) => station.tx_retries = Some(*retries),
                _ => {}
            }
        }

        Some(station)
    }

    /// nl80211 reports bitrates in units of 100 kbit/s; the 32-bit field replaces the 16-bit
    /// one above 6.5 Gbit/s
    fn bitrate_kbps(rate: &[Nl80211RateInfo]) -> Option<u32> {
        let bitrate32 = rate.iter().find_map(|info| match info {
            Nl80211RateInfo::Bitrate32(rate) => Some(*rate),
            _ => None,
        });
        let bitrate16 = rate.iter().find_map(|info| match info {
            Nl80211RateInfo::Bitrate(rate) => Some(u32::from(*rate)),
            _ => None,
        });

        bitrate32.or(bitrate16).map(|rate| rate.saturating_mul(100))
    }
}
