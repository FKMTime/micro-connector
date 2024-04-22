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
    tag_channels: HashMap<u64, tokio::sync::oneshot::Sender<UnixResponseData>>,
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

            // TODO: if write_all fails, maybe retry???
            inner.socket_channel.send(req)?;
            //inner.stream.write_all(&buf).await?;
        }

        if tag.is_some() {
            // TODO: add better errors (for timeout, and recv error)
            let resp = tokio::time::timeout(UNIX_TIMEOUT, resp_rx).await??;
            return Ok(Some(resp));
        }

        Ok(None)
    }
}

async fn socket_task(socket_path: String, rx: UnboundedReceiver<UnixRequest>) {
    tokio::task::spawn(async move {
        loop {
            let res = inner_socket_task(&socket_path, &rx).await;
            if let Err(e) = res {
                tracing::error!("Socket task err: {e:?}");
                _ = tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });
}

async fn inner_socket_task(socket_path: &str, rx: &UnboundedReceiver<UnixRequest>) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).await?;

    loop {
        let recv = read_until_null(&mut stream).await?;
        tracing::trace!("Recv: {}", String::from_utf8_lossy(&recv));
    }
    /*
    tokio::select! {
    }
    */
}

async fn read_until_null(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0x00 {
            break;
        }

        buf.push(byte);
    }

    Ok(buf)
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixResponse {
    pub tag: Option<u64>,

    #[serde(flatten)]
    pub data: UnixResponseData,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
pub enum UnixResponseData {
    CompetitorInfo {
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
