use anyhow::Result;
use std::{
    path::Path,
    time::{Duration, SystemTime},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
use unix_utils::{request::UnixRequest, response::UnixResponse};

#[no_mangle]
pub unsafe extern "Rust" fn hil_log(tag: &str, content: String) {
    println!("[{tag}] {content}");
}

#[tokio::main]
async fn main() -> Result<()> {
    _ = dotenvy::dotenv();
    tracing_subscriber::fmt::init();

    let socket_path = std::env::var("SOCKET_PATH").unwrap_or("/tmp/sock/socket.sock".to_string());
    let socket_dir = Path::new(&socket_path).parent().unwrap();
    _ = tokio::fs::create_dir_all(socket_dir).await;
    _ = tokio::fs::remove_file(&socket_path).await;

    let tests_root = tokio::fs::read("tests.json")
        .await
        .map_err(|_| anyhow::anyhow!("tests.json doesnt exists!"))?;
    let tests_root: hil_processor::structs::TestsRoot = serde_json::from_slice(&tests_root)?;

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("Unix listener started on path {socket_path}!");
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut state = hil_processor::HilState {
            devices: Vec::new(),
            tests: tests_root,
            should_send_status: true,
            get_ms: || {
                return SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
            },

            completed_count: 0,
        };

        if let Ok(out) = state.process_packet(None) {
            for packet in out {
                _ = send_raw_resp(&mut stream, packet).await;
            }
        }

        let mut buf = Vec::with_capacity(512);
        loop {
            let res = tokio::time::timeout(
                Duration::from_millis(50),
                read_until_null(&mut stream, &mut buf),
            )
            .await;

            let packet: Option<UnixRequest> = if let Ok(Ok(bytes)) = res {
                serde_json::from_slice(&bytes[..])?
            } else {
                None
            };

            if let Ok(out) = state.process_packet(packet) {
                for packet in out {
                    _ = send_raw_resp(&mut stream, packet).await;
                }
            }

            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

async fn send_raw_resp(stream: &mut UnixStream, data: UnixResponse) -> Result<()> {
    stream.write_all(&serde_json::to_vec(&data)?).await?;
    stream.write_u8(0x00).await?;

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
