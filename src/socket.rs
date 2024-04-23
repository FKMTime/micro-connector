use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::{mpsc::UnboundedReceiver, OnceCell, RwLock},
};

const UNIX_TIMEOUT: Duration = Duration::from_millis(2500);

type InnerRwLock = Arc<RwLock<SocketInner>>;
#[derive(Debug, Clone)]
pub struct Socket {
    inner: OnceCell<InnerRwLock>,
}

#[derive(Debug)]
pub struct SocketInner {
    //stream: UnixStream,
    socket_channel: tokio::sync::mpsc::UnboundedSender<UnixRequest>,
    tag_channels: HashMap<u64, tokio::sync::oneshot::Sender<Option<UnixResponseData>>>,
}

impl Socket {
    pub const fn const_new() -> Self {
        Socket {
            inner: OnceCell::const_new(),
        }
    }

    pub async fn init(&self, socket_path: &str) -> Result<()> {
        let (socket_channel, rx) = tokio::sync::mpsc::unbounded_channel();

        let inner = Arc::new(RwLock::new(SocketInner {
            //stream: UnixStream::connect(socket_path).await?,
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

    pub async fn send_request(
        &self,
        tag: Option<u64>,
        data: UnixRequestData,
    ) -> Result<Option<UnixResponseData>> {
        let req = UnixRequest { tag, data };
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
        tag: u64,
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

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixResponse {
    pub tag: Option<u64>,

    #[serde(flatten)]
    pub data: Option<UnixResponseData>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all_fields = "camelCase")]
pub enum UnixResponseData {
    WifiSettings {
        wifi_ssid: String,
        wifi_password: String,
    },
    PersonInfo {
        id: String,
        registrant_id: Option<i64>,
        name: String,
        wca_id: Option<String>,
        country_iso2: Option<String>,
        gender: String,
        can_compete: bool,
    },
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixRequest {
    pub tag: Option<u64>,

    #[serde(flatten)]
    pub data: UnixRequestData,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
pub enum UnixRequestData {
    PersonInfo { card_id: u128 },
}
