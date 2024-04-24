use anyhow::Result;
use std::{collections::HashMap, sync::Arc, time::Duration};
use structs::{UnixRequest, UnixRequestData, UnixResponse, UnixResponseData};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{mpsc::UnboundedReceiver, OnceCell, RwLock},
};

use crate::structs::SharedCompetitionStatus;

use self::structs::UnixError;

pub mod api;
pub mod structs;

const UNIX_TIMEOUT: Duration = Duration::from_millis(2500);

type InnerRwLock = Arc<RwLock<SocketInner>>;
#[derive(Debug, Clone)]
pub struct Socket {
    inner: OnceCell<InnerRwLock>,
}

#[derive(Debug)]
pub struct SocketInner {
    //stream: UnixStream,
    comp_status: SharedCompetitionStatus,
    socket_channel: tokio::sync::mpsc::UnboundedSender<UnixRequest>,
    tag_channels: HashMap<u32, tokio::sync::oneshot::Sender<Option<UnixResponseData>>>,
}

impl Socket {
    pub const fn const_new() -> Self {
        Socket {
            inner: OnceCell::const_new(),
        }
    }

    pub async fn init(
        &self,
        socket_path: &str,
        comp_status: SharedCompetitionStatus,
    ) -> Result<()> {
        let (socket_channel, rx) = tokio::sync::mpsc::unbounded_channel();

        let inner = Arc::new(RwLock::new(SocketInner {
            comp_status,
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

        if let Some(resp) = resp {
            return match resp {
                UnixResponseData::Error {
                    message,
                    should_reset_time,
                } => Err(UnixError {
                    message,
                    should_reset_time,
                }),
                _ => Ok(resp),
            };
        }

        Err(UnixError {
            message: "No response".to_string(),
            should_reset_time: false,
        })
    }

    /// Request without response (non-waiting)
    pub async fn send_async_request(&self, data: UnixRequestData) -> Result<(), UnixError> {
        _ = self.send_request(None, data).await.map_err(|_| UnixError {
            message: "Send failed".to_string(),
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
        let req = UnixRequest { tag, data };
        tracing::trace!("Sending Unix request: {req:?}");

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
            // TODO: Maybe retry???
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

                tracing::trace!("Unix response: {}", String::from_utf8_lossy(&recv));
                let resp: UnixResponse = serde_json::from_slice(&recv)?;
                tracing::debug!("Unix Response Data: {resp:?}");

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
            let mut comp_status = inner.comp_status.write().await;

            comp_status.should_update = status.should_update;
            let mut changed = false;

            // delete devices that are not in the new status
            let devices_clone = comp_status.devices_settings.clone();
            for (k, _) in devices_clone {
                if !status.rooms.iter().any(|r| r.devices.contains(&k)) {
                    comp_status.devices_settings.remove(&k);
                    changed = true;
                }
            }

            for room in status.rooms {
                for device in room.devices {
                    let old = comp_status.devices_settings.insert(
                        device,
                        crate::structs::CompetitionDeviceSettings {
                            use_inspection: room.use_inspection,
                        },
                    );

                    if old.is_none() || old.unwrap().use_inspection != room.use_inspection {
                        changed = true;
                    }
                }
            }

            if changed {
                _ = comp_status.broadcaster.send(());
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
