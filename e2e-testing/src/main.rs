use anyhow::Result;
use rand::Rng;
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use structs::{CompetitorInfo, SharedSenders, State, TestsRoot};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc::UnboundedReceiver, OnceCell, RwLock},
};
use unix_utils::{
    request::{UnixRequest, UnixRequestData},
    response::{CompetitionStatusResp, Room, UnixResponse, UnixResponseData},
    TestPacketData,
};

use crate::structs::TestStep;

mod structs;

pub static UNIX_SENDER: OnceCell<tokio::sync::mpsc::UnboundedSender<UnixResponse>> =
    OnceCell::const_new();

#[tokio::main]
async fn main() -> Result<()> {
    let socket_path = std::env::var("SOCKET_PATH").unwrap_or("/tmp/sock/socket.sock".to_string());
    let socket_dir = Path::new(&socket_path).parent().unwrap();
    _ = tokio::fs::create_dir_all(socket_dir).await;
    _ = tokio::fs::remove_file(&socket_path).await;

    let tests_root = tokio::fs::read("tests.json").await?;
    let tests_root: TestsRoot = serde_json::from_slice(&tests_root)?;

    let mut state = State {
        devices: vec![],
        senders: Arc::new(RwLock::new(HashMap::new())),
        tests: Arc::new(RwLock::new(tests_root)),
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    UNIX_SENDER.set(tx)?;

    let listener = UnixListener::bind(socket_path)?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        let res = handle_stream(&mut stream, &mut state, &mut rx).await;
        println!("res: {res:?}");
    }
}

async fn handle_stream(
    stream: &mut UnixStream,
    state: &mut State,
    rx: &mut UnboundedReceiver<UnixResponse>,
) -> Result<()> {
    send_status_resp(stream, &state.devices).await?;

    let mut buf = Vec::with_capacity(512);
    loop {
        tokio::select! {
            res = read_until_null(stream, &mut buf) => {
                let bytes = res?;
                let packet: UnixRequest = serde_json::from_slice(&bytes[..])?;

                let mut print_log = true;
                match packet.data {
                    UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                        state.devices.push(esp_id);
                        send_status_resp(stream, &state.devices).await?;
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;

                        let tests = state.tests.read().await.clone();
                        new_test_sender(&esp_id, state.senders.clone(), tests).await?;
                    }
                    UnixRequestData::PersonInfo { ref card_id } => {
                        let card_id: u64 = card_id.parse()?;
                        let tests_root = state.tests.read().await;
                        let competitor = tests_root.cards.get(&card_id);

                        let resp = match competitor {
                            Some(competitor) => UnixResponseData::PersonInfoResp {
                                id: card_id.to_string(),
                                registrant_id: Some(competitor.registrant_id),
                                name: competitor.name.to_string(),
                                wca_id: Some(competitor.wca_id.to_string()),
                                country_iso2: Some("PL".to_string()),
                                gender: "Male".to_string(),
                                can_compete: competitor.can_compete,
                            },
                            None => UnixResponseData::Error {
                                message: "Competitor not found".to_string(),
                                should_reset_time: false,
                            },
                        };

                        send_resp(stream, resp, packet.tag, competitor.is_none()).await?;
                    }
                    UnixRequestData::EnterAttempt { esp_id, .. } => {
                        send_senders_data(&state.senders, &esp_id, packet.data.clone()).await?;
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                    UnixRequestData::Snapshot(ref data) => {
                        send_senders_data(&state.senders, &data.esp_id, packet.data.clone()).await?;
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                    UnixRequestData::UpdateBatteryPercentage { .. } => {
                        print_log = false;
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                    _ => {
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                }

                if print_log {
                    println!("{packet:?}");
                }
            }
            Some(recv) = rx.recv() => {
                send_raw_resp(stream, recv).await?;
            }
        }
    }
}

async fn new_test_sender(esp_id: &u32, senders: SharedSenders, tests: TestsRoot) -> Result<()> {
    let esp_id = *esp_id;

    tokio::task::spawn(async move {
        let res = test_sender(esp_id, senders, tests).await;
        println!("{res:?}");
    });

    Ok(())
}

async fn test_sender(esp_id: u32, senders: SharedSenders, tests: TestsRoot) -> Result<()> {
    let unix_tx = UNIX_SENDER.get().expect("UNIX_SENDER not set!");
    let mut rx = spawn_new_sender(&senders, esp_id).await?;

    unix_tx.send(UnixResponse {
        error: None,
        tag: None,
        data: Some(UnixResponseData::TestPacket {
            esp_id,
            data: TestPacketData::Start,
        }),
    })?;

    unix_tx.send(UnixResponse {
        error: None,
        tag: None,
        data: Some(UnixResponseData::TestPacket {
            esp_id,
            data: TestPacketData::ResetState,
        }),
    })?;

    for test in tests.tests {
        println!("Running test: {}", test.name);
        let random_time: u64 = rand::thread_rng().gen_range(501..123042);

        // TODO: separate function to easily tell where it errored!
        for step in test.steps {
            println!("Running step: {step:?}");

            match step {
                TestStep::Sleep(ms) => {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    continue; // to skip sleep_between after
                }
                TestStep::ResetState => {
                    unix_tx.send(UnixResponse {
                        error: None,
                        tag: None,
                        data: Some(UnixResponseData::TestPacket {
                            esp_id,
                            data: TestPacketData::ResetState,
                        }),
                    })?;
                }
                TestStep::SolveTime(time) => {
                    unix_tx.send(UnixResponse {
                        error: None,
                        tag: None,
                        data: Some(UnixResponseData::TestPacket {
                            esp_id,
                            data: TestPacketData::SolveTime(time),
                        }),
                    })?;
                }
                TestStep::SolveTimeRng => {
                    unix_tx.send(UnixResponse {
                        error: None,
                        tag: None,
                        data: Some(UnixResponseData::TestPacket {
                            esp_id,
                            data: TestPacketData::SolveTime(random_time),
                        }),
                    })?;
                }
                TestStep::Snapshot => {
                    unix_tx.send(UnixResponse {
                        error: None,
                        tag: None,
                        data: Some(UnixResponseData::TestPacket {
                            esp_id,
                            data: TestPacketData::Snapshot,
                        }),
                    })?;

                    let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await?;
                    println!("Snapshot data: {recv:?}");
                }
                TestStep::ScanCard(card_id) => {
                    unix_tx.send(UnixResponse {
                        error: None,
                        tag: None,
                        data: Some(UnixResponseData::TestPacket {
                            esp_id,
                            data: TestPacketData::ScanCard(card_id),
                        }),
                    })?;
                }
                TestStep::Button { ref name, time } => {
                    let pins = tests.buttons.get(name);
                    if let Some(pins) = pins {
                        unix_tx.send(UnixResponse {
                            error: None,
                            tag: None,
                            data: Some(UnixResponseData::TestPacket {
                                esp_id,
                                data: TestPacketData::ButtonPress {
                                    pins: pins.to_owned(),
                                    press_time: time,
                                },
                            }),
                        })?;
                    } else {
                        println!("Wrong button name!");
                    }
                }
                TestStep::VerifySolveTime {
                    time,
                    penalty: penalty_to_check,
                } => {
                    let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("Shouldnt be none"))?;

                    if let UnixRequestData::EnterAttempt { value, penalty, .. } = recv {
                        let time_to_check = time.unwrap_or(random_time) / 10;
                        if value != time_to_check {
                            anyhow::bail!(
                                "Wrong time value! Real: {value} Expected: {time_to_check}"
                            )
                        }

                        if penalty != penalty_to_check {
                            anyhow::bail!(
                                "Wrong penalty value! Real: {penalty} Expected: {penalty_to_check}"
                            )
                        }
                    } else {
                        anyhow::bail!("Wrong packet, cant verify solve time!")
                    }
                }
                _ => {
                    println!("Step not coded... {step:?}");
                }
            }

            tokio::time::sleep(Duration::from_millis(test.sleep_between)).await;
        }

        if tests.dump_state_after_test {
            unix_tx.send(UnixResponse {
                error: None,
                tag: None,
                data: Some(UnixResponseData::TestPacket {
                    esp_id,
                    data: TestPacketData::Snapshot,
                }),
            })?;

            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await?;
            println!("Snapshot data: {recv:?}");
        }
    }

    Ok(())
}

async fn send_resp(
    stream: &mut UnixStream,
    data: UnixResponseData,
    tag: Option<u32>,
    error: bool,
) -> Result<()> {
    let packet = UnixResponse {
        tag,
        error: Some(error),
        data: Some(data),
    };
    send_raw_resp(stream, packet).await?;

    Ok(())
}

async fn send_raw_resp(stream: &mut UnixStream, data: UnixResponse) -> Result<()> {
    stream.write_all(&serde_json::to_vec(&data)?).await?;
    stream.write_u8(0x00).await?;

    Ok(())
}

async fn send_status_resp(stream: &mut UnixStream, device_store: &Vec<u32>) -> Result<()> {
    let status_packet = UnixResponse {
        tag: None,
        error: None,
        data: Some(UnixResponseData::ServerStatus(CompetitionStatusResp {
            should_update: true,
            devices: device_store.to_vec(),
            rooms: vec![Room {
                id: "dsa".to_string(),
                name: "room 1".to_string(),
                devices: device_store.to_vec(),
                use_inspection: true,
            }],
        })),
    };

    send_raw_resp(stream, status_packet).await?;
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

pub async fn send_senders_data(
    senders: &SharedSenders,
    esp_id: &u32,
    data: UnixRequestData,
) -> Result<()> {
    let senders = senders.read().await;
    if let Some(sender) = senders.get(esp_id) {
        sender.send(data)?;
    }

    Ok(())
}

pub async fn spawn_new_sender(
    senders: &SharedSenders,
    esp_id: u32,
) -> Result<UnboundedReceiver<UnixRequestData>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut senders = senders.write().await;
    senders.insert(esp_id, tx);

    Ok(rx)
}
