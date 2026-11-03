use crate::{
    structs::{SharedAppState, TimerPacket, TimerPacketInner},
    updater::FirmwareMetadata,
};
use anyhow::Result;
use base64::Engine;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{OnceCell, RwLock, mpsc::UnboundedReceiver},
};
use unix_utils::{
    UnixError,
    request::{UnixRequest, UnixRequestData},
    response::{TranslationLocale, TranslationRecord, UnixResponse, UnixResponseData},
};

pub mod api;

const UNIX_TIMEOUT: Duration = Duration::from_millis(7500);

type InnerRwLock = Arc<RwLock<SocketInner>>;
#[derive(Debug, Clone)]
pub struct Socket {
    inner: OnceCell<InnerRwLock>,
}

#[derive(Debug)]
pub struct SocketInner {
    //stream: UnixStream,
    state: SharedAppState,
    socket_channel: tokio::sync::mpsc::UnboundedSender<UnixRequest>,
    tag_channels: HashMap<u32, PendingReply>,
}

#[derive(Debug)]
struct PendingReply {
    tx: tokio::sync::oneshot::Sender<Option<UnixResponseData>>,
    /// Logged at trace instead of info, see [`is_quiet`]
    quiet: bool,
}

/// Takes a pending reply out of `tag_channels` when its request stops waiting without one:
/// on timeout, or when the caller's future is dropped (e.g. the device's connection task is
/// aborted). Nothing else would ever remove it.
struct PendingReplyGuard {
    inner: InnerRwLock,
    tag: Option<u32>,
}

impl Drop for PendingReplyGuard {
    fn drop(&mut self) {
        let Some(tag) = self.tag else {
            return;
        };

        // drop can't wait for the lock, so whoever holds it gets a task to do it later
        if let Ok(mut inner) = self.inner.try_write() {
            inner.tag_channels.remove(&tag);
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let inner = self.inner.clone();
            runtime.spawn(async move {
                inner.write().await.tag_channels.remove(&tag);
            });
        }
    }
}

/// Requests sent every second by every station: logging them at info would write
/// tens of lines a second to the router's flash.
fn is_quiet(data: &UnixRequestData) -> bool {
    matches!(data, UnixRequestData::CurrentTimeInfo { .. })
}

fn log_response(quiet: bool, raw: &[u8], resp: &UnixResponse) {
    let raw = String::from_utf8_lossy(raw);
    if quiet {
        tracing::trace!(file = "unix", "Received unix response (JSON): {raw}");
        tracing::trace!(file = "unix", "Received unix response: {resp:?}");
    } else {
        tracing::info!(file = "unix", "Received unix response (JSON): {raw}");
        tracing::info!(file = "unix", "Received unix response: {resp:?}");
    }
}

impl Socket {
    pub const fn const_new() -> Self {
        Socket {
            inner: OnceCell::const_new(),
        }
    }

    pub async fn init(&self, socket_path: &str, state: SharedAppState) -> Result<()> {
        let (socket_channel, rx) = tokio::sync::mpsc::unbounded_channel();

        let inner = Arc::new(RwLock::new(SocketInner {
            state: state.clone(),
            socket_channel,
            tag_channels: HashMap::new(),
        }));
        self.inner.set(inner)?;

        socket_task(socket_path.to_string(), rx, state).await;
        Ok(())
    }

    pub async fn get_inner(&self) -> Result<InnerRwLock> {
        self.inner
            .get()
            .ok_or_else(|| anyhow::anyhow!("Inner not set! Call .init() function!"))
            .cloned()
    }

    /// Request with response (waiting)
    pub async fn send_tagged_request(
        &self,
        data: UnixRequestData,
    ) -> Result<UnixResponseData, UnixError> {
        // i think its not likely that tags would generate the same in short amount of time
        let tag: u32 = rand::random();

        let resp = self
            .send_request(Some(tag), data)
            .await
            .map_err(|_| UnixError {
                message: "Send failed".to_string(),
                should_reset_time: false,
            })?;

        match resp {
            Some(UnixResponseData::Error {
                message,
                should_reset_time,
            }) => Err(UnixError {
                message,
                should_reset_time,
            }),
            Some(data) => Ok(data),
            None => Ok(UnixResponseData::Empty),
        }
    }

    /// Request without response (non-waiting)
    pub async fn send_async_request(&self, data: UnixRequestData) -> Result<(), UnixError> {
        _ = self.send_request(None, data).await.map_err(|_| UnixError {
            message: "Send failed 2".to_string(),
            should_reset_time: false,
        })?;

        Ok(())
    }

    // TODO: implement resending if something fails
    async fn send_request(
        &self,
        tag: Option<u32>,
        data: UnixRequestData,
    ) -> Result<Option<UnixResponseData>> {
        let quiet = is_quiet(&data);
        let req = UnixRequest { tag, data };

        if quiet {
            tracing::trace!(file = "unix", "Sending Unix Request: {req:?}");
        } else {
            tracing::info!(file = "unix", "Sending Unix Request: {req:?}");
        }

        let inner = self.get_inner().await?;
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

        // inside parens to unlock after send!
        {
            let mut inner = inner.write().await;
            inner.socket_channel.send(req)?;
            if let Some(tag) = tag {
                inner
                    .tag_channels
                    .insert(tag, PendingReply { tx: resp_tx, quiet });
            }
        }

        if let Some(tag) = tag {
            let mut guard = PendingReplyGuard {
                inner,
                tag: Some(tag),
            };

            // TODO: add better errors (for timeout, and recv error)
            let resp = tokio::time::timeout(UNIX_TIMEOUT, resp_rx).await;
            if resp.is_ok() {
                // the reply (or a dropped sender) means it was already taken out
                guard.tag = None;
            }
            return Ok(resp??);
        }

        Ok(None)
    }

    pub async fn send_resp_to_channel(
        &self,
        tag: u32,
        raw: &[u8],
        resp: UnixResponse,
    ) -> Result<()> {
        let inner = self.get_inner().await?;
        let pending = inner.write().await.tag_channels.remove(&tag);

        log_response(pending.as_ref().is_some_and(|p| p.quiet), raw, &resp);
        if let Some(pending) = pending {
            _ = pending.tx.send(resp.data);
        }

        Ok(())
    }
}

pub(crate) async fn socket_task(
    socket_path: String,
    mut rx: UnboundedReceiver<UnixRequest>,
    state: SharedAppState,
) {
    tokio::task::spawn(async move {
        loop {
            let res = inner_socket_task(&socket_path, &mut rx, &state).await;
            if let Err(e) = res {
                tracing::error!("Socket task err: {e:?}");
                _ = tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });
}

async fn inner_socket_task(
    socket_path: &str,
    rx: &mut UnboundedReceiver<UnixRequest>,
    state: &SharedAppState,
) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).await?;

    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        tokio::select! {
            recv = read_until_null(&mut stream, &mut buf) => {
                let recv = recv?;

                let resp: UnixResponse = serde_json::from_slice(&recv)?;

                if let Some(tag) = resp.tag {
                    super::UNIX_SOCKET.send_resp_to_channel(tag, &recv, resp).await?;
                } else {
                    log_response(false, &recv, &resp);
                    if let Some(data) = resp.data {
                        process_untagged_response(data, state).await?;
                    }
                }
            }
            Some(recv) = rx.recv() => {
                let bytes = serde_json::to_vec(&recv)?;

                stream.write_all(&bytes).await?;
                stream.write_u8(0x00).await?; // null byte separator
            }
        }
    }
}

async fn process_untagged_response(data: UnixResponseData, state: &SharedAppState) -> Result<()> {
    match data {
        UnixResponseData::CustomMessage {
            esp_id,
            line1,
            line2,
        } => {
            let packet = TimerPacket {
                tag: None,
                data: TimerPacketInner::CustomMessage { line1, line2 },
            };

            let inner = crate::UNIX_SOCKET.get_inner().await?;
            let inner = inner.read().await;
            let state = &inner.state;

            state.send_timer_packet(esp_id, packet).await?;
        }
        UnixResponseData::ServerStatus(status) => {
            let mut inner_state = state.inner.write().await;

            // remove all unicode weirdness
            let translations: Vec<TranslationLocale> = status
                .translations
                .into_iter()
                .map(|l| TranslationLocale {
                    locale: l.locale,
                    translations: l
                        .translations
                        .into_iter()
                        .map(|t| TranslationRecord {
                            key: t.key,
                            translation: unidecode::unidecode(&t.translation),
                        })
                        .collect(),
                })
                .collect();

            let mut changed = inner_state.should_update != status.should_update
                || inner_state.locales != translations
                || inner_state.default_locale != status.default_locale
                || inner_state.auto_setup != status.auto_setup
                || inner_state.fkm_token != status.fkm_token
                || inner_state.secure_rfid != status.secure_rfid
                || inner_state.sound_enabled != status.sound_enabled;

            inner_state.server_status_received = true;
            inner_state.should_update = status.should_update;
            inner_state.locales = translations;
            inner_state.default_locale = status.default_locale;
            inner_state.auto_setup = status.auto_setup;
            inner_state.fkm_token = status.fkm_token;
            inner_state.secure_rfid = status.secure_rfid;
            inner_state.sound_enabled = status.sound_enabled;

            for device in &status.devices {
                let device_settings = crate::structs::DeviceSettings {
                    sign_key: device.sign_key,
                };

                let old = inner_state
                    .devices_settings
                    .insert(device.esp_id, device_settings.clone());

                if old != Some(device_settings) {
                    changed = true;
                }
            }

            for (k, _) in inner_state.devices_settings.clone() {
                if !status.devices.iter().any(|d| d.esp_id == k) {
                    inner_state.devices_settings.remove(&k);
                    changed = true;
                }
            }

            drop(inner_state);
            if changed {
                _ = state.device_settings_broadcast().await;
            }
        }
        UnixResponseData::IncidentResolved {
            esp_id,
            should_scan_cards,
            attempt,
        } => {
            let packet = TimerPacket {
                tag: None,
                data: TimerPacketInner::DelegateResponse {
                    should_scan_cards,
                    solve_time: attempt.value.map(|x| x * 10),
                    penalty: attempt.penalty,
                },
            };

            let inner = crate::UNIX_SOCKET.get_inner().await?;
            let inner = inner.read().await;
            let state = &inner.state;

            state.send_timer_packet(esp_id, packet).await?;
        }
        UnixResponseData::TestPacket { esp_id, data } => {
            let inner = crate::UNIX_SOCKET.get_inner().await?;
            let inner = inner.read().await;
            let state = &inner.state;

            state
                .send_timer_packet(
                    esp_id,
                    TimerPacket {
                        tag: None,
                        data: TimerPacketInner::TestPacket(data),
                    },
                )
                .await?;
        }
        UnixResponseData::UploadFirmware {
            file_name,
            file_data,
        } => {
            let data = base64::prelude::BASE64_STANDARD.decode(file_data);
            let Ok(data) = data else {
                tracing::error!("ForceUpdate Base64 parse error!");
                return Ok(());
            };

            let metadata = FirmwareMetadata::from_file(&file_name, &data).await;
            let Ok(metadata) = metadata else {
                tracing::error!(
                    "ForceUpdate File metadata read error! {:?}",
                    metadata.expect_err("")
                );
                return Ok(());
            };

            let (Ok(hardware), Ok(firmware), Ok(version)) = (
                core::str::from_utf8(&metadata.hardware).map(|x| x.trim_end_matches('\0')),
                core::str::from_utf8(&metadata.firmware).map(|x| x.trim_end_matches('\0')),
                core::str::from_utf8(&metadata.version).map(|x| x.trim_end_matches('\0')),
            ) else {
                tracing::error!("Wrong firmware metadata utf8.");
                return Ok(());
            };

            let version = crate::updater::Version::from_str(version);
            state
                .force_update(
                    hardware.to_string(),
                    crate::updater::Firmware {
                        data,
                        version,
                        build_time: metadata.build_time,
                        firmware: firmware.to_string(),
                    },
                )
                .await?;
        }
        UnixResponseData::SetDeviceSettings { devices, volume } => {
            for esp_id in devices {
                state
                    .send_timer_packet(
                        esp_id,
                        TimerPacket {
                            tag: None,
                            data: TimerPacketInner::SetDeviceSettings { volume },
                        },
                    )
                    .await?;
            }
        }
        _ => {}
    }

    Ok(())
}

async fn read_until_null(stream: &mut UnixStream, buf: &mut Vec<u8>) -> Result<Vec<u8>> {
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0x00 {
            let ret = buf.to_owned();
            buf.clear();

            return Ok(ret);
        }

        buf.push(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_time_info() -> UnixRequestData {
        UnixRequestData::CurrentTimeInfo {
            esp_id: 0xA1B2,
            time: None,
            inspection: None,
            competitor: None,
            group_id: None,
            session_id: None,
            timer_running: None,
            inspection_running: None,
        }
    }

    /// Not `UNIX_SOCKET`: that one can only be initialised once and the handler tests own it.
    async fn socket() -> (Socket, UnboundedReceiver<UnixRequest>) {
        let (socket_channel, rx) = tokio::sync::mpsc::unbounded_channel();
        let socket = Socket::const_new();
        let inner = SocketInner {
            state: SharedAppState::new(false).await,
            socket_channel,
            tag_channels: HashMap::new(),
        };
        socket.inner.set(Arc::new(RwLock::new(inner))).unwrap();

        (socket, rx)
    }

    async fn pending_replies(socket: &Socket) -> usize {
        socket
            .get_inner()
            .await
            .unwrap()
            .read()
            .await
            .tag_channels
            .len()
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_request_leaves_no_reply_channel() {
        let (socket, mut rx) = socket().await;

        assert!(
            socket
                .send_tagged_request(current_time_info())
                .await
                .is_err()
        );
        assert!(rx.recv().await.unwrap().tag.is_some());
        assert_eq!(pending_replies(&socket).await, 0);
    }

    async fn spawn_pending_request(
        socket: &Arc<Socket>,
        rx: &mut UnboundedReceiver<UnixRequest>,
    ) -> tokio::task::JoinHandle<Result<UnixResponseData, UnixError>> {
        let request = tokio::spawn({
            let socket = socket.clone();
            async move { socket.send_tagged_request(current_time_info()).await }
        });
        assert!(rx.recv().await.unwrap().tag.is_some());
        assert_eq!(pending_replies(socket).await, 1);

        request
    }

    #[tokio::test]
    async fn abandoned_request_leaves_no_reply_channel() {
        let (socket, mut rx) = socket().await;
        let socket = Arc::new(socket);

        let request = spawn_pending_request(&socket, &mut rx).await;
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());

        assert_eq!(pending_replies(&socket).await, 0);
    }

    #[tokio::test]
    async fn request_abandoned_while_the_lock_is_held_leaves_no_reply_channel() {
        let (socket, mut rx) = socket().await;
        let socket = Arc::new(socket);

        let request = spawn_pending_request(&socket, &mut rx).await;
        let inner = socket.get_inner().await.unwrap();
        let reader = inner.read().await;
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(reader.tag_channels.len(), 1);
        drop(reader);

        let cleaned_up = async {
            while pending_replies(&socket).await > 0 {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(1), cleaned_up)
            .await
            .expect("reply channel left behind");
    }

    #[tokio::test]
    async fn answered_request_leaves_no_reply_channel() {
        let (socket, mut rx) = socket().await;
        let socket = Arc::new(socket);

        let request = tokio::spawn({
            let socket = socket.clone();
            async move { socket.send_tagged_request(current_time_info()).await }
        });
        let tag = rx.recv().await.unwrap().tag.unwrap();
        assert_eq!(pending_replies(&socket).await, 1);

        let resp = UnixResponse {
            error: None,
            tag: Some(tag),
            data: Some(UnixResponseData::Empty),
        };
        socket.send_resp_to_channel(tag, b"{}", resp).await.unwrap();

        assert!(matches!(
            request.await.unwrap(),
            Ok(UnixResponseData::Empty)
        ));
        assert_eq!(pending_replies(&socket).await, 0);
    }

    #[test]
    fn only_current_time_info_is_quiet() {
        assert!(is_quiet(&current_time_info()));
        assert!(!is_quiet(&UnixRequestData::DeviceConnected {
            esp_id: 0xA1B2,
            firmware_version: "v3.5.0".to_string(),
        }));
        assert!(!is_quiet(&UnixRequestData::AutoSetupSettings));
    }
}
