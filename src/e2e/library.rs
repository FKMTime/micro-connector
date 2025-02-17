use crate::structs::{TestStep, TestsRoot};
use anyhow::Result;
use rand::Rng as _;
use unix_utils::{
    request::{UnixRequest, UnixRequestData},
    response::{CompetitionStatusResp, UnixResponse, UnixResponseData},
    TestPacketData,
};

pub struct HilState {
    pub devices: Vec<HilDeviceQueue>,
    pub tests: TestsRoot,

    pub should_send_status: bool,

    pub get_ms: fn() -> u64,
}

#[derive(Clone)]
pub struct HilDeviceQueue {
    pub id: u32,
    pub back_packet: Option<UnixRequestData>,
    pub next_step_time: u64,

    pub current_test: Option<usize>,
    pub current_step: usize,
    pub wait_for_ack: bool,

    pub expected_time: u64,
    pub remove_after: bool,
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
                        current_step: 0,
                        next_step_time: 0,
                        wait_for_ack: false,

                        expected_time: 0,
                        remove_after: false,
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
                        dev.wait_for_ack = false;
                    }
                }
                _ => {
                    send_resp(&mut responses, UnixResponseData::Empty, packet.tag, false);
                }
            }

            tracing::trace!("{packet:?}");
        }

        self.devices = self
            .devices
            .iter()
            .filter(|d| !d.remove_after)
            .cloned()
            .collect();

        for device in &mut self.devices {
            if device.wait_for_ack {
                let timeout_reached = (self.get_ms)() >= device.next_step_time + 5000;
                if timeout_reached {
                    tracing::error!("TIMEOUT REACHED!");
                    device.remove_after = true;
                }

                continue;
            }

            if device.next_step_time > (self.get_ms)() {
                continue;
            }

            // get new test (currently first one)
            if device.current_test.is_none() {
                let next_idx: usize = rand::rng().random_range(0..self.tests.tests.len());
                device.current_test = Some(next_idx);
                device.current_step = 0;
                device.next_step_time = (self.get_ms)();
            }

            let Some(current_step) = &self.tests.tests[device.current_test.unwrap_or(0)]
                .steps
                .get(device.current_step)
            else {
                tracing::info!("test end?");
                device.current_test = None;
                continue;
            };

            tracing::trace!("current_step: {current_step:?}");
            //tracing::info!(" > Running step: {step:?} (esp_id: {esp_id})");

            match current_step {
                TestStep::Sleep(ms) => {
                    device.current_step += 1;
                    device.next_step_time = (self.get_ms)() + ms;
                    continue; // to skip sleep_between after
                }
                TestStep::ResetState => {
                    send_test_packet(&mut responses, device.id, TestPacketData::ResetState);
                    device.current_step += 1;
                    device.next_step_time = (self.get_ms)();
                }
                TestStep::SolveTime(time) => {
                    send_test_packet(
                        &mut responses,
                        device.id,
                        TestPacketData::StackmatTime(*time),
                    );

                    device.expected_time = *time;
                    device.current_step += 1;
                    device.next_step_time = (self.get_ms)() + *time + 100;
                }
                TestStep::SolveTimeRng => {
                    let mut random_time: u64 = rand::rng().random_range(501..14132);
                    if random_time == device.expected_time {
                        random_time += 1;
                    }

                    send_test_packet(
                        &mut responses,
                        device.id,
                        TestPacketData::StackmatTime(random_time),
                    );

                    device.expected_time = random_time;
                    device.current_step += 1;
                    device.next_step_time = (self.get_ms)() + random_time + 100;
                }
                TestStep::Snapshot => {
                    // TODO: add Snapshots to new firmware
                    /*
                    send_test_packet(&unix_tx, rx, esp_id, TestPacketData::Snapshot).await?;

                    let recv = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await?;
                    tracing::debug!("Snapshot data: {recv:?}");
                    */
                }
                TestStep::ScanCard(card_id) => {
                    send_test_packet(
                        &mut responses,
                        device.id,
                        TestPacketData::ScanCard(*card_id),
                    );
                    device.current_step += 1;
                    device.next_step_time = (self.get_ms)();
                }
                TestStep::Button { ref name, time } => {
                    let pin = self.tests.buttons.get(name);
                    if let Some(&pin) = pin {
                        send_test_packet(
                            &mut responses,
                            device.id,
                            TestPacketData::ButtonPress {
                                pin,
                                press_time: *time,
                            },
                        );

                        device.current_step += 1;
                        device.next_step_time = (self.get_ms)() + *time;
                    } else {
                        tracing::error!("Wrong button name");
                        device.remove_after = true;
                    }
                }
                TestStep::VerifySolveTime {
                    time,
                    penalty: penalty_to_check,
                } => {
                    let Some(ref back_packet) = device.back_packet else {
                        let timeout_reached = (self.get_ms)() >= device.next_step_time + 5000;
                        if timeout_reached {
                            tracing::error!("TIMEOUT REACHED!");
                            device.remove_after = true;
                        }

                        continue;
                    };

                    if let UnixRequestData::EnterAttempt {
                        value,
                        penalty,
                        is_delegate,
                        ..
                    } = back_packet
                    {
                        let time_to_check = time.unwrap_or(device.expected_time) / 10;
                        if *value != time_to_check {
                            tracing::error!(
                                "Wrong time value! Real: {value} Expected: {time_to_check}"
                            );
                            device.remove_after = true;
                        }

                        if *penalty != *penalty_to_check {
                            tracing::error!(
                                "Wrong penalty value! Real: {penalty} Expected: {penalty_to_check}"
                            );
                            device.remove_after = true;
                        }

                        if *is_delegate {
                            tracing::error!(
                                "Wrong is_delegate value! Real: {is_delegate} Expected: false"
                            );
                            device.remove_after = true;
                        }
                    } else {
                        tracing::error!("Wrong packet, cant verify solve time!");
                        device.remove_after = true;
                    }

                    device.current_step += 1;
                    device.back_packet = None;
                }
                TestStep::VerifyDelegateSent => {
                    let Some(ref back_packet) = device.back_packet else {
                        let timeout_reached = (self.get_ms)() >= device.next_step_time + 5000;
                        if timeout_reached {
                            tracing::error!("TIMEOUT REACHED!");
                            device.remove_after = true;
                        }

                        continue;
                    };

                    if let UnixRequestData::EnterAttempt { is_delegate, .. } = back_packet {
                        if !is_delegate {
                            tracing::error!(
                                "Wrong is_delegate value! Real: {is_delegate} Expected: true"
                            );
                            device.remove_after = true;
                        }
                    } else {
                        tracing::error!("Wrong packet, cant verify delegate!");
                        device.remove_after = true;
                    }

                    device.current_step += 1;
                    device.back_packet = None;
                }
                TestStep::DelegateResolve {
                    should_scan_cards,
                    penalty,
                    value,
                } => {
                    let packet = UnixResponse {
                        error: None,
                        tag: None,
                        data: Some(UnixResponseData::IncidentResolved {
                            esp_id: device.id,
                            should_scan_cards: *should_scan_cards,
                            attempt: unix_utils::response::IncidentAttempt {
                                session_id: "".to_string(),
                                penalty: *penalty,
                                value: *value,
                            },
                        }),
                    };

                    responses.push(packet);
                    device.current_step += 1;
                }
                #[allow(unreachable_patterns)]
                _ => {
                    tracing::error!("Step not matched! {current_step:?}");
                }
            }

            device.next_step_time +=
                self.tests.tests[device.current_test.unwrap_or(0)].sleep_between;

            // timeout = 5s
            let timeout_reached = (self.get_ms)() >= device.next_step_time + 5000;
            if timeout_reached {
                tracing::error!("TIMEOUT REACHED!");
                device.remove_after = true;
                continue;
            }
        }

        Ok(responses)
    }
}

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

fn send_test_packet(responses: &mut Vec<UnixResponse>, esp_id: u32, data: TestPacketData) {
    let packet = UnixResponse {
        error: None,
        tag: None,
        data: Some(UnixResponseData::TestPacket { esp_id, data }),
    };

    responses.push(packet);
}
