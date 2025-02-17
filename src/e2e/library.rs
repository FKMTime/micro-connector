use anyhow::Result;
use unix_utils::{
    request::{UnixRequest, UnixRequestData},
    response::{CompetitionStatusResp, UnixResponse, UnixResponseData},
    TestPacketData,
};

use crate::structs::{TestData, TestStep, TestsRoot};

pub struct HilState {
    pub devices: Vec<HilDeviceQueue>,
    pub tests: TestsRoot,

    pub should_send_status: bool,

    pub get_ms: fn() -> u64,
}

pub struct HilDeviceQueue {
    pub id: u32,
    pub back_packet: Option<UnixRequestData>,
    pub next_step_time: u64,

    pub current_test: Option<usize>,
    pub test_step_idx: usize,

    pub last_rng_time: u64,
}

impl HilState {
    pub fn process_packet(&mut self, packet: Option<UnixRequest>) -> Result<Vec<UnixResponse>> {
        let mut responses = Vec::new();
        if self.should_send_status {
            send_status_resp(&mut responses, &self);
            self.should_send_status = false;
        }

        if let Some(packet) = packet {
            match packet.data {
                UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                    let device = HilDeviceQueue {
                        id: esp_id,
                        current_test: None,
                        back_packet: None,
                        test_step_idx: 0,
                        next_step_time: 0,

                        last_rng_time: 0,
                    };
                    self.devices.push(device);

                    send_status_resp(&mut responses, &self);
                    send_resp(&mut responses, UnixResponseData::Empty, packet.tag, false);
                }
                UnixRequestData::PersonInfo {
                    ref card_id,
                    esp_id: _,
                } => {
                    let card_id: u64 = card_id.parse()?;
                    let competitor = self.tests.cards.get(&card_id);

                    let resp = match competitor {
                        Some(competitor) => UnixResponseData::PersonInfoResp {
                            id: card_id.to_string(),
                            registrant_id: Some(competitor.registrant_id),
                            name: competitor.name.to_string(),
                            wca_id: Some(competitor.wca_id.to_string()),
                            country_iso2: Some("PL".to_string()),
                            gender: "Male".to_string(),
                            can_compete: competitor.can_compete,
                            possible_groups: self
                                .tests
                                .groups
                                .clone()
                                .into_iter()
                                .filter(|x| competitor.groups.contains(&x.group_id))
                                .collect(),
                        },
                        None => UnixResponseData::Error {
                            message: "Competitor not found".to_string(),
                            should_reset_time: false,
                        },
                    };

                    send_resp(&mut responses, resp, packet.tag, competitor.is_none());
                }
                UnixRequestData::EnterAttempt { esp_id, .. } => {
                    let dev = self.devices.iter_mut().find(|d| d.id == esp_id);
                    if let Some(dev) = dev {
                        dev.back_packet = Some(packet.data.clone());
                        dev.next_step_time = (self.get_ms)() + 300; // run next step after 300ms
                    }

                    send_resp(&mut responses, UnixResponseData::Empty, packet.tag, false);
                }
                UnixRequestData::Snapshot(ref data) => {
                    let dev = self.devices.iter_mut().find(|d| d.id == data.esp_id);
                    if let Some(dev) = dev {
                        dev.back_packet = Some(packet.data.clone());
                    }

                    send_resp(&mut responses, UnixResponseData::Empty, packet.tag, false);
                }
                UnixRequestData::UpdateBatteryPercentage { .. } => {
                    send_resp(&mut responses, UnixResponseData::Empty, packet.tag, false);
                }
                UnixRequestData::TestAck { esp_id } => {
                    let dev = self.devices.iter_mut().find(|d| d.id == esp_id);
                    if let Some(dev) = dev {
                        dev.back_packet = Some(packet.data.clone());
                    }
                }
                _ => {
                    send_resp(&mut responses, UnixResponseData::Empty, packet.tag, false);
                }
            }

            tracing::trace!("{packet:?}");
        }

        for device in &mut self.devices {
            if device.next_step_time > (self.get_ms)() {
                continue;
            }

            // get new test (currently first one)
            if device.current_test.is_none() {
                device.current_test = Some(0);
            }

            //let current_step =
        }

        Ok(responses)
    }
}

/*
async fn run_step(
    unix_tx: &UnboundedSender<UnixResponse>,
    rx: &mut UnboundedReceiver<UnixRequestData>,
    esp_id: u32,
    tests: &TestsRoot,
    test_index: usize,
    step_index: usize,
    random_time: u64,
    last_time: &mut u64,
) -> Result<()> {
    let test = &tests.tests[test_index];
    let step = &test.steps[step_index];

    tracing::info!(" > Running step: {step:?} (esp_id: {esp_id})");

    match step {
        TestStep::Sleep(ms) => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
            return Ok(()); // to skip sleep_between after
        }
        TestStep::ResetState => {
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::ResetState).await?;
        }
        TestStep::SolveTime(time) => {
            *last_time = *time;
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::StackmatTime(*time)).await?;

            tokio::time::sleep(Duration::from_millis(*time + 100)).await;
        }
        TestStep::SolveTimeRng => {
            *last_time = random_time;
            send_test_packet(
                &unix_tx,
                rx,
                esp_id,
                TestPacketData::StackmatTime(random_time),
            )
            .await?;

            tokio::time::sleep(Duration::from_millis(random_time + 100)).await;
        }
        TestStep::Snapshot => {
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::Snapshot).await?;

            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await?;
            tracing::debug!("Snapshot data: {recv:?}");
        }
        TestStep::ScanCard(card_id) => {
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::ScanCard(*card_id)).await?;
        }
        TestStep::Button { ref name, time } => {
            let pin = tests.buttons.get(name);
            if let Some(&pin) = pin {
                send_test_packet(
                    &unix_tx,
                    rx,
                    esp_id,
                    TestPacketData::ButtonPress {
                        pin,
                        press_time: *time,
                    },
                )
                .await?;

                tokio::time::sleep(Duration::from_millis(*time)).await;
            } else {
                tracing::error!("Wrong button name");
            }
        }
        TestStep::VerifySolveTime {
            time,
            penalty: penalty_to_check,
        } => {
            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Shouldnt be none"))?;

            if let UnixRequestData::EnterAttempt {
                value,
                penalty,
                is_delegate,
                ..
            } = recv
            {
                let time_to_check = time.unwrap_or(random_time) / 10;
                if value != time_to_check {
                    anyhow::bail!("Wrong time value! Real: {value} Expected: {time_to_check}")
                }

                if penalty != *penalty_to_check {
                    anyhow::bail!(
                        "Wrong penalty value! Real: {penalty} Expected: {penalty_to_check}"
                    )
                }

                if is_delegate {
                    anyhow::bail!("Wrong is_delegate value! Real: {is_delegate} Expected: false")
                }
            } else {
                anyhow::bail!("Wrong packet, cant verify solve time!")
            }
        }
        TestStep::VerifyDelegateSent => {
            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Shouldnt be none"))?;

            if let UnixRequestData::EnterAttempt { is_delegate, .. } = recv {
                if !is_delegate {
                    anyhow::bail!("Wrong is_delegate value! Real: {is_delegate} Expected: true")
                }
            } else {
                anyhow::bail!("Wrong packet, cant verify delegate!")
            }
        }
        TestStep::DelegateResolve {
            should_scan_cards,
            penalty,
            value,
        } => {
            unix_tx.send(UnixResponse {
                error: None,
                tag: None,
                data: Some(UnixResponseData::IncidentResolved {
                    esp_id,
                    should_scan_cards: *should_scan_cards,
                    attempt: unix_utils::response::IncidentAttempt {
                        session_id: "".to_string(),
                        penalty: *penalty,
                        value: *value,
                    },
                }),
            })?;
        }

        #[allow(unreachable_patterns)]
        _ => {
            tracing::error!("Step not matched! {step:?}");
        }
    }

    Ok(())
}
*/

fn send_resp(
    responses: &mut Vec<UnixResponse>,
    data: UnixResponseData,
    tag: Option<u32>,
    error: bool,
) {
    let packet = UnixResponse {
        tag,
        error: Some(error),
        data: Some(data),
    };

    responses.push(packet);
}

fn send_status_resp(responses: &mut Vec<UnixResponse>, state: &HilState) {
    send_resp(
        responses,
        UnixResponseData::ServerStatus(CompetitionStatusResp {
            should_update: true,
            devices: state.devices.iter().map(|d| d.id).collect(),
            translations: Vec::new(),
            default_locale: "en".to_string(),
        }),
        None,
        false,
    );
}

/*
async fn handle_stream(
    stream: &mut UnixStream,
    state: &mut State,
    rx: &mut UnboundedReceiver<UnixResponse>,
    send_dev_rx: &mut UnboundedReceiver<()>,
) -> Result<()> {
    {
        send_status_resp(stream, &state.devices.read().await.to_vec()).await?;
    }

    let mut buf = Vec::with_capacity(512);
    loop {
        tokio::select! {
            res = read_until_null(stream, &mut buf) => {
                let bytes = res?;
                let packet: UnixRequest = serde_json::from_slice(&bytes[..])?;

                match packet.data {
                    UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                        {
                            let mut devices = state.devices.write().await;
                            devices.push(esp_id);

                            send_status_resp(stream, &devices.to_vec()).await?;
                        }

                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;

                        let tests = state.tests.read().await.clone();
                        new_test_sender(&esp_id, state.devices.clone(), state.senders.clone(), tests).await?;
                    }
                    UnixRequestData::PersonInfo { ref card_id, esp_id: _ } => {
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
                                possible_groups: tests_root.groups
                                    .clone()
                                    .into_iter()
                                    .filter(|x| competitor.groups.contains(&x.group_id))
                                    .collect(),
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
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                    UnixRequestData::TestAck { esp_id } => {
                        send_senders_data(&state.senders, &esp_id, packet.data.clone()).await?;
                        //send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                    _ => {
                        send_resp(stream, UnixResponseData::Empty, packet.tag, false).await?;
                    }
                }

                tracing::trace!("{packet:?}");
            }
            Some(recv) = rx.recv() => {
                send_raw_resp(stream, recv).await?;
            }
            _ = send_dev_rx.recv() => {
                send_status_resp(stream, &state.devices.read().await.to_vec()).await?;
            }
        }
    }
}

async fn new_test_sender(
    esp_id: &u32,
    devices: Arc<RwLock<Vec<u32>>>,
    senders: SharedSenders,
    tests: TestsRoot,
) -> Result<()> {
    let esp_id = *esp_id;

    tokio::task::spawn(async move {
        tracing::info!("Starting new test sender for esp with id: {esp_id}");

        let res = test_sender(esp_id, devices, senders, tests).await;
        if let Err(e) = res {
            tracing::error!("Test sender error: {e:?}");
        }
    });

    Ok(())
}

async fn test_sender(
    esp_id: u32,
    devices: Arc<RwLock<Vec<u32>>>,
    senders: SharedSenders,
    tests: TestsRoot,
) -> Result<()> {
    let unix_tx = UNIX_SENDER.get().expect("UNIX_SENDER not set!");
    let mut rx = spawn_new_sender(&senders, esp_id).await?;

    send_test_packet(&unix_tx, &mut rx, esp_id, TestPacketData::ResetState).await?;

    let mut counter = 0;
    let mut prev_idx: Option<usize> = None;
    let mut last_time = 0;
    loop {
        let next_idx: usize = rand::rng().random_range(0..tests.tests.len());
        if let Some(prev_idx) = prev_idx {
            if prev_idx == next_idx {
                continue;
            }
        }

        prev_idx = Some(next_idx);
        let res = run_test(&unix_tx, &mut rx, esp_id, &tests, next_idx, &mut last_time).await;
        if let Err(e) = res {
            tracing::error!("Run test error: {e:?}");
            {
                let mut dev = devices.write().await;
                let index = dev
                    .iter()
                    .enumerate()
                    .find(|(_, e)| **e == esp_id)
                    .map(|(i, _)| i);

                if let Some(index) = index {
                    dev.remove(index);
                }

                _ = SEND_DEVICES.get().unwrap().send(());
            }

            break Ok(());
        }

        counter += 1;
        tracing::info!("==================================");
        tracing::info!("Device ({esp_id}) COUNT: {counter}");
        tracing::info!("==================================");
    }
}

async fn run_test(
    unix_tx: &UnboundedSender<UnixResponse>,
    rx: &mut UnboundedReceiver<UnixRequestData>,
    esp_id: u32,
    tests: &TestsRoot,
    test_index: usize,
    last_time: &mut u64,
) -> Result<()> {
    let test = &tests.tests[test_index];

    tracing::info!("Running test: {} (esp: {esp_id})", test.name);
    let mut random_time: u64 = rand::rng().random_range(501..27132);
    if *last_time == random_time {
        random_time += 1;
    }

    for step_idx in 0..test.steps.len() {
        let step = run_step(
            unix_tx,
            rx,
            esp_id,
            tests,
            test_index,
            step_idx,
            random_time,
            last_time,
        )
        .await;

        if let Err(e) = step {
            tracing::error!("Step error: {e:?}");
            anyhow::bail!("Step error");
        }

        tokio::time::sleep(Duration::from_millis(test.sleep_between)).await;
    }

    if tests.dump_state_after_test {
        send_test_packet(&unix_tx, rx, esp_id, TestPacketData::Snapshot).await?;

        let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await?;
        tracing::debug!("Snapshot data: {recv:?}");
    }

    Ok(())
}

async fn run_step(
    unix_tx: &UnboundedSender<UnixResponse>,
    rx: &mut UnboundedReceiver<UnixRequestData>,
    esp_id: u32,
    tests: &TestsRoot,
    test_index: usize,
    step_index: usize,
    random_time: u64,
    last_time: &mut u64,
) -> Result<()> {
    let test = &tests.tests[test_index];
    let step = &test.steps[step_index];

    tracing::info!(" > Running step: {step:?} (esp_id: {esp_id})");

    match step {
        TestStep::Sleep(ms) => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
            return Ok(()); // to skip sleep_between after
        }
        TestStep::ResetState => {
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::ResetState).await?;
        }
        TestStep::SolveTime(time) => {
            *last_time = *time;
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::StackmatTime(*time)).await?;

            tokio::time::sleep(Duration::from_millis(*time + 100)).await;
        }
        TestStep::SolveTimeRng => {
            *last_time = random_time;
            send_test_packet(
                &unix_tx,
                rx,
                esp_id,
                TestPacketData::StackmatTime(random_time),
            )
            .await?;

            tokio::time::sleep(Duration::from_millis(random_time + 100)).await;
        }
        TestStep::Snapshot => {
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::Snapshot).await?;

            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await?;
            tracing::debug!("Snapshot data: {recv:?}");
        }
        TestStep::ScanCard(card_id) => {
            send_test_packet(&unix_tx, rx, esp_id, TestPacketData::ScanCard(*card_id)).await?;
        }
        TestStep::Button { ref name, time } => {
            let pin = tests.buttons.get(name);
            if let Some(&pin) = pin {
                send_test_packet(
                    &unix_tx,
                    rx,
                    esp_id,
                    TestPacketData::ButtonPress {
                        pin,
                        press_time: *time,
                    },
                )
                .await?;

                tokio::time::sleep(Duration::from_millis(*time)).await;
            } else {
                tracing::error!("Wrong button name");
            }
        }
        TestStep::VerifySolveTime {
            time,
            penalty: penalty_to_check,
        } => {
            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Shouldnt be none"))?;

            if let UnixRequestData::EnterAttempt {
                value,
                penalty,
                is_delegate,
                ..
            } = recv
            {
                let time_to_check = time.unwrap_or(random_time) / 10;
                if value != time_to_check {
                    anyhow::bail!("Wrong time value! Real: {value} Expected: {time_to_check}")
                }

                if penalty != *penalty_to_check {
                    anyhow::bail!(
                        "Wrong penalty value! Real: {penalty} Expected: {penalty_to_check}"
                    )
                }

                if is_delegate {
                    anyhow::bail!("Wrong is_delegate value! Real: {is_delegate} Expected: false")
                }
            } else {
                anyhow::bail!("Wrong packet, cant verify solve time!")
            }
        }
        TestStep::VerifyDelegateSent => {
            let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Shouldnt be none"))?;

            if let UnixRequestData::EnterAttempt { is_delegate, .. } = recv {
                if !is_delegate {
                    anyhow::bail!("Wrong is_delegate value! Real: {is_delegate} Expected: true")
                }
            } else {
                anyhow::bail!("Wrong packet, cant verify delegate!")
            }
        }
        TestStep::DelegateResolve {
            should_scan_cards,
            penalty,
            value,
        } => {
            unix_tx.send(UnixResponse {
                error: None,
                tag: None,
                data: Some(UnixResponseData::IncidentResolved {
                    esp_id,
                    should_scan_cards: *should_scan_cards,
                    attempt: unix_utils::response::IncidentAttempt {
                        session_id: "".to_string(),
                        penalty: *penalty,
                        value: *value,
                    },
                }),
            })?;
        }

        #[allow(unreachable_patterns)]
        _ => {
            tracing::error!("Step not matched! {step:?}");
        }
    }

    Ok(())
}

async fn send_test_packet(
    unix_tx: &UnboundedSender<UnixResponse>,
    rx: &mut UnboundedReceiver<UnixRequestData>,
    esp_id: u32,
    data: TestPacketData,
) -> Result<()> {
    unix_tx.send(UnixResponse {
        error: None,
        tag: None,
        data: Some(UnixResponseData::TestPacket { esp_id, data }),
    })?;

    let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("Shouldnt be none"))?;

    if let UnixRequestData::TestAck { esp_id } = recv {
        if esp_id != esp_id {
            anyhow::bail!("Wrong esp_id in response!");
        }
    } else {
        anyhow::bail!("Wrong packet, cant verify test ack!");
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
            translations: Vec::new(),
            default_locale: "en".to_string(),
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
*/
