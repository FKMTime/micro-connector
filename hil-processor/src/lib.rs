use crate::structs::TestStep;
use anyhow::Result;
use rand::Rng as _;
use unix_utils::{
    request::{UnixRequest, UnixRequestData},
    response::{CompetitionStatusResp, UnixResponse, UnixResponseData},
    TestPacketData,
};

pub use structs::{HilDevice, HilState};
pub mod snapshot;
pub mod structs;

#[allow(unused_macros)]
#[macro_export]
macro_rules! info {
    ($self:ident, $($arg:tt)+) => (
        $self.log("INFO", format!($($arg)+));
    );
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! debug {
    ($self:ident, $($arg:tt)+) => (
        $self.log("DEBUG", format!($($arg)+));
    );
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! error {
    ($self:ident, $($arg:tt)+) => (
        $self.log("ERROR", format!($($arg)+));
    );
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! warn {
    ($self:ident, $($arg:tt)+) => (
        $self.log("WARN", format!($($arg)+));
    );
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! trace {
    ($self:ident, $($arg:tt)+) => (
        $self.log("TRACE", format!($($arg)+));
    );
}

impl HilDevice {
    pub fn new(id: u32) -> HilDevice {
        HilDevice {
            id,
            last_snapshot: None,
            current_test: None,
            back_packet: None,
            current_step: 0,
            next_step_time: 0,
            wait_for_ack: false,

            last_test: usize::MAX,

            last_solve_time: 0,
            completed_count: 0,
        }
    }
}

impl HilState {
    pub fn feed(&mut self, packet: Option<UnixRequest>) -> Result<()> {
        if let Some(packet) = packet {
            trace!(self, "Feed: {packet:?}");

            match packet.data {
                UnixRequestData::RequestToConnectDevice { esp_id, .. } => {
                    let dev = self.devices.iter().find(|d| d.id == esp_id);
                    if dev.is_some() {
                        return Ok(());
                    }

                    let device = HilDevice::new(esp_id);
                    self.devices.push(device);

                    self.send_status_resp();
                    self.send_resp(UnixResponseData::Empty, packet.tag, false);
                    self.send_resp(
                        UnixResponseData::TestPacket {
                            esp_id,
                            data: TestPacketData::HardStateReset,
                        },
                        None,
                        false,
                    );
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

                    self.send_resp(resp, packet.tag, competitor.is_none());
                }
                UnixRequestData::EnterAttempt { esp_id, .. } => {
                    let dev = self.devices.iter_mut().find(|d| d.id == esp_id);
                    if let Some(dev) = dev {
                        dev.back_packet = Some(packet.data.clone());
                        dev.next_step_time = (self.get_ms)(); // run next step after 300ms
                    }

                    self.send_resp(UnixResponseData::Empty, packet.tag, false);
                }
                UnixRequestData::UpdateBatteryPercentage { .. } => {
                    self.send_resp(UnixResponseData::Empty, packet.tag, false);
                }
                UnixRequestData::TestAck { esp_id, snapshot } => {
                    let dev = self.devices.iter_mut().find(|d| d.id == esp_id);
                    if let Some(dev) = dev {
                        dev.last_snapshot = Some(snapshot);
                        dev.wait_for_ack = false;
                        dev.next_step_time = (self.get_ms)() + 250;
                    }
                }
                _ => {
                    self.send_resp(UnixResponseData::Empty, packet.tag, false);
                }
            }
        }

        Ok(())
    }

    pub fn process(&mut self) -> Result<Vec<UnixResponse>> {
        if self.should_send_status {
            self.send_status_resp();
            self.should_send_status = false;
        }

        for i in (0..self.devices.len()).rev() {
            let mut device = self.devices[i].clone();
            let dev_res = self.process_device(&mut device);
            if let Err(e) = dev_res {
                error!(self, "process_device error: {e:?}");

                let device = &self.devices[i];
                self.send_device_custom_message(
                    device.id,
                    format!(
                        "HIL Error {}/{}",
                        device.current_test.unwrap_or(0),
                        device.current_step
                    ),
                    format!("{e:?}",),
                );

                self.devices.remove(i);
                self.should_send_status = true;
            } else {
                self.devices[i] = device;
            }
        }

        Ok(self.packet_queue.drain(..).collect())
    }

    fn process_device(&mut self, device: &mut HilDevice) -> Result<(), ()> {
        if device.wait_for_ack {
            let timeout_reached = (self.get_ms)() >= device.next_step_time + 5000;
            if timeout_reached {
                return Err(());
            }

            return Ok(());
        }

        if device.next_step_time > (self.get_ms)() {
            return Ok(());
        }

        if device.current_test.is_none() {
            let mut next_idx: usize = rand::rng().random_range(0..self.tests.tests.len());
            if next_idx == device.last_test {
                next_idx += 1;
                if next_idx >= self.tests.tests.len() {
                    next_idx = 0;
                }
            }

            info!(
                self,
                "Startin new test({}): {}", device.id, self.tests.tests[next_idx].name
            );

            device.current_test = Some(next_idx);
            device.current_step = 0;
            device.next_step_time = (self.get_ms)();
            device.last_test = next_idx;
        }

        let Some(current_step) = &self.tests.tests[device.current_test.unwrap_or(0)]
            .steps
            .get(device.current_step)
            .cloned()
        else {
            self.completed_count += 1;
            device.completed_count += 1;
            info!(
                self,
                "Test end! ({}) [{}] [{}]", device.id, device.completed_count, self.completed_count
            );

            device.current_test = None;
            return Ok(());
        };

        trace!(
            self,
            " > Running step: {current_step:?} (esp_id: {})",
            device.id
        );

        match current_step {
            TestStep::Sleep(ms) => {
                device.current_step += 1;
                device.next_step_time = (self.get_ms)() + ms;
                return Ok(());
            }
            TestStep::ResetState => {
                self.send_test_packet(device.id, TestPacketData::ResetState);

                device.wait_for_ack = true;
                device.current_step += 1;
                device.next_step_time = (self.get_ms)();
            }
            TestStep::SolveTime => {
                let random_time: u64 = rand::rng().random_range(501..14132);

                self.send_test_packet(device.id, TestPacketData::StackmatTime(random_time));

                device.wait_for_ack = true;
                device.last_solve_time = random_time;
                device.current_step += 1;
                device.next_step_time = (self.get_ms)() + random_time;
            }
            TestStep::ScanCard(card_id) => {
                self.send_test_packet(device.id, TestPacketData::ScanCard(*card_id));

                device.wait_for_ack = true;
                device.current_step += 1;
                device.next_step_time = (self.get_ms)();
            }
            TestStep::Button { name, time, ack } => {
                let pin = self.tests.buttons.get(name);
                if let Some(&pin) = pin {
                    self.send_test_packet(
                        device.id,
                        TestPacketData::ButtonPress {
                            pin,
                            press_time: *time,
                        },
                    );

                    if *ack != Some(false) {
                        device.wait_for_ack = true;
                    }

                    device.current_step += 1;
                    device.next_step_time = (self.get_ms)() + *time;
                } else {
                    error!(self, "Wrong button name");
                    return Err(());
                }
            }
            TestStep::VerifySend {
                time: time_to_check,
                penalty: penalty_to_check,
                delegate: was_delegate,
            } => {
                let Some(ref back_packet) = device.back_packet else {
                    let timeout_reached = (self.get_ms)() >= device.next_step_time + 5000;
                    if timeout_reached {
                        return Err(());
                    }

                    return Ok(());
                };

                if let UnixRequestData::EnterAttempt {
                    value,
                    penalty,
                    is_delegate,
                    ..
                } = back_packet
                {
                    if let Some(mut time_to_check) = time_to_check {
                        if time_to_check == -1 {
                            time_to_check = device.last_solve_time as i64;
                        }

                        time_to_check /= 10;
                        if *value != time_to_check as u64 {
                            error!(
                                self,
                                "Wrong time value! Real: {value} Expected: {time_to_check}"
                            );

                            return Err(());
                        }
                    }

                    if let Some(penalty_to_check) = penalty_to_check {
                        if *penalty != *penalty_to_check {
                            error!(
                                self,
                                "Wrong penalty value! Real: {penalty} Expected: {penalty_to_check}"
                            );

                            return Err(());
                        }
                    }

                    if *is_delegate != *was_delegate {
                        error!(
                            self,
                            "Wrong is_delegate value! Real: {is_delegate} Expected: false"
                        );

                        return Err(());
                    }
                } else {
                    error!(self, "Wrong packet, cant verify solve time!");
                    return Err(());
                }

                device.current_step += 1;
                device.back_packet = None;
            }
            TestStep::DelegateResolve {
                should_scan_cards,
                penalty,
                value,
            } => {
                if let Some(time) = value {
                    device.last_solve_time = *time;
                }

                self.send_resp(
                    UnixResponseData::IncidentResolved {
                        esp_id: device.id,
                        should_scan_cards: *should_scan_cards,
                        attempt: unix_utils::response::IncidentAttempt {
                            session_id: "".to_string(),
                            penalty: *penalty,
                            value: value.map(|v| v / 10),
                        },
                    },
                    None,
                    false,
                );

                device.current_step += 1;
            }
            TestStep::VerifySnapshot(queries) => {
                for query in queries {
                    if snapshot::snapshot_dsl_check(&self, device, &query) != Ok(true) {
                        error!(self, "HIL query failed: {query}");
                        return Err(());
                    }
                }

                device.current_step += 1;
                device.next_step_time = (self.get_ms)();
            }
            #[allow(unreachable_patterns)]
            _ => {
                error!(self, "Step not matched! {current_step:?}");
                return Err(());
            }
        }

        device.next_step_time += self.tests.tests[device.current_test.unwrap_or(0)].sleep_between;
        Ok(())
    }

    fn send_device_custom_message(&mut self, esp_id: u32, line1: String, line2: String) {
        self.send_resp(
            UnixResponseData::CustomMessage {
                esp_id,
                line1,
                line2,
            },
            None,
            false,
        );
    }

    pub fn send_resp(&mut self, data: UnixResponseData, tag: Option<u32>, error: bool) {
        let packet = UnixResponse {
            tag,
            error: Some(error),
            data: Some(data),
        };

        self.packet_queue.push(packet);
    }

    pub fn send_status_resp(&mut self) {
        self.send_resp(
            UnixResponseData::ServerStatus(CompetitionStatusResp {
                should_update: self.status.should_update,
                devices: self.devices.iter().map(|d| d.id).collect(),
                translations: self.status.translations.clone(),
                default_locale: self.status.default_locale.clone(),
            }),
            None,
            false,
        );
    }

    pub fn send_test_packet(&mut self, esp_id: u32, data: TestPacketData) {
        let packet = UnixResponse {
            error: None,
            tag: None,
            data: Some(UnixResponseData::TestPacket { esp_id, data }),
        };

        self.packet_queue.push(packet);
    }

    pub fn process_initial_status_devices(&mut self) {
        for &dev_id in &self.status.devices {
            let device = HilDevice::new(dev_id);
            self.devices.push(device);
        }
    }

    fn log(&self, tag: &str, msg: String) {
        (self.log_fn)(tag, msg);
    }
}
