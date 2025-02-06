use crate::structs::{SharedAppState, TimerPacket, TimerPacketInner};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{mpsc::UnboundedReceiver, OnceCell, RwLock},
};
use unix_utils::{
    request::{UnixRequest, UnixRequestData},
    response::{UnixResponse, UnixResponseData},
    UnixError,
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
    tag_channels: HashMap<u32, tokio::sync::oneshot::Sender<Option<UnixResponseData>>>,
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
            state,
            socket_channel,
            tag_channels: HashMap::new(),
        }));
        self.inner.set(inner)?;

        socket_task(socket_path.to_string(), rx).await;
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
        let req = UnixRequest {
            tag,
            data: data.clone(),
        };

        tracing::info!(file = "unix", "Sending Unix Request: {req:?}");

        let inner = self.get_inner().await?;
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

        // inside parens to unlock after send!
        {
            let mut inner = inner.write().await;
            if let Some(tag) = tag {
                inner.tag_channels.insert(tag, resp_tx);
            }

            inner.socket_channel.send(req)?;
        }

        if tag.is_some() {
            // TODO: add better errors (for timeout, and recv error)
            let resp = tokio::time::timeout(UNIX_TIMEOUT, resp_rx).await??;
            return Ok(resp);
        }

        Ok(None)
    }

    pub async fn send_resp_to_channel(
        &self,
        tag: u32,
        resp: Option<UnixResponseData>,
    ) -> Result<()> {
        let inner = self.get_inner().await?;
        let mut inner = inner.write().await;

        if let Some(chan) = inner.tag_channels.remove(&tag) {
            _ = chan.send(resp);
        }

        Ok(())
    }
}

async fn socket_task(socket_path: String, mut rx: UnboundedReceiver<UnixRequest>) {
    tokio::task::spawn(async move {
        loop {
            let res = inner_socket_task(&socket_path, &mut rx).await;
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
) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).await?;

    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        tokio::select! {
            recv = read_until_null(&mut stream, &mut buf) => {
                let recv = recv?;

                let resp: UnixResponse = serde_json::from_slice(&recv)?;
                tracing::info!(file = "unix", "Received unix response (JSON): {}", core::str::from_utf8(&recv)?);
                tracing::info!(file = "unix", "Received unix response: {resp:?}");

                if let Some(tag) = resp.tag {
                    super::UNIX_SOCKET.send_resp_to_channel(tag, resp.data).await?;
                } else if let Some(data) = resp.data {
                    process_untagged_response(data).await?;
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

async fn process_untagged_response(data: UnixResponseData) -> Result<()> {
    match data {
        UnixResponseData::ServerStatus(status) => {
            let inner = crate::UNIX_SOCKET.get_inner().await?;
            let inner = inner.read().await;
            let mut inner_state = inner.state.inner.write().await;

            inner_state.should_update = status.should_update;
            let mut changed = false;

            for &device in &status.devices {
                let room_settings = crate::structs::CompetitionDeviceSettings {};
                let old = inner_state
                    .devices_settings
                    .insert(device, room_settings.clone());

                if old.as_ref() != Some(&room_settings) {
                    changed = true;
                }
            }

            for (k, _) in inner_state.devices_settings.clone() {
                if !status.devices.contains(&k) {
                    inner_state.devices_settings.remove(&k);
                    changed = true;
                }
            }

            if changed {
                _ = inner.state.device_settings_broadcast().await;
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
                    solve_time: attempt.value.map(|x| x * 10), // from cs to ms
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
