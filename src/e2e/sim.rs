//! Simulation mode (`E2E_SIM=1`): plays FKMTime's backend for the server-driven flows instead of
//! running HIL steps, so device simulators can test them end to end.
//!
//! Follows FKMTime's `resultFromDevice.service.ts` / `personForDevice.service.ts`: attempts are
//! stored per session id, re-sent sessions are rejected (or confirm a resolved incident), delegate
//! calls become unresolved incidents, extra attempts replace the original, cutoffs, time limits
//! and finished rounds are enforced. Rounds come from the optional `rounds` map in `tests.json`
//! (`{"333-r1": {"attempts": 5, "cutoff": {"attempts": 2, "value": 1000}, "limit": 2500}}`, values
//! in centiseconds); a round without an entry is unlimited.
//!
//! The delegate is scripted over the control socket (`E2E_CONTROL`, JSON lines, one reply line
//! per command): `{"cmd":"attempts"}`, `{"cmd":"resolve","sessionId":..,"status":"resolved"|
//! "standard"|"extra_given","value":..,"penalty":..,"noCards":false}`, `{"cmd":"delete",
//! "sessionId":..}`, `{"cmd":"drop_reply","espId":..}` (the next attempt of that device is entered
//! but never answered). Every request with its answer, push and command is appended to `E2E_LOG`
//! (JSON lines: `{"ms","req","resp"}`, `{"ms","push"}`, `{"ms","ctl","result"}`).

use anyhow::Result;
use hil_processor::structs::TestsRoot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc::{UnboundedSender, unbounded_channel},
};
use unix_utils::{
    request::{UnixRequest, UnixRequestData},
    response::{
        CompetitionStatusDevice, CompetitionStatusResp, IncidentAttempt, UnixResponse,
        UnixResponseData,
    },
};

#[derive(Deserialize, Default, Clone)]
struct SimConfig {
    #[serde(default)]
    rounds: HashMap<String, RoundInfo>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct RoundInfo {
    /// 0 = unlimited
    #[serde(default)]
    attempts: u32,
    cutoff: Option<Cutoff>,
    /// centiseconds; an attempt at or over it becomes a DNF
    limit: Option<u64>,
}

#[derive(Deserialize, Clone, Copy)]
struct Cutoff {
    attempts: usize,
    value: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
enum Status {
    Standard,
    Unresolved,
    Resolved,
    ExtraGiven,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Attempt {
    session_id: String,
    esp_id: u32,
    competitor_id: u64,
    judge_id: Option<u64>,
    group_id: String,
    round_id: String,
    attempt_number: u32,
    extra: bool,
    status: Status,
    /// centiseconds
    value: u64,
    value_ms: u64,
    penalty: i64,
    inspection_time: i64,
    replaced_by: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(
    tag = "cmd",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Command {
    Attempts,
    Resolve {
        session_id: String,
        status: Status,
        value: Option<u64>,
        penalty: Option<i64>,
        #[serde(default)]
        no_cards: bool,
    },
    Delete {
        session_id: String,
    },
    DropReply {
        esp_id: u32,
    },
}

type Reply = Result<UnixResponseData, (&'static str, bool)>;

struct Fkm {
    tests: TestsRoot,
    rounds: HashMap<String, RoundInfo>,
    devices: Vec<CompetitionStatusDevice>,
    attempts: Vec<Attempt>,
    drop_replies: Vec<u32>,
    log: std::fs::File,
    /// the connected micro-connector (FKMTime pushes to every socket; one is enough here)
    out: Option<UnboundedSender<UnixResponse>>,
}

fn round_of(group_id: &str) -> &str {
    group_id.split("-g").next().unwrap_or(group_id)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn ok(message: &str) -> Reply {
    Ok(UnixResponseData::EnterAttemptResp {
        message: message.to_string(),
    })
}

impl Fkm {
    fn write_log(&mut self, mut entry: Value) {
        entry["ms"] = json!(now_ms());
        if let Err(e) = writeln!(self.log, "{entry}") {
            tracing::error!("e2e log write failed: {e:?}");
        }
    }

    /// Untagged packet to the micro-connector; false if none is connected (lost, like FKMTime's).
    fn push(&mut self, data: UnixResponseData) -> bool {
        let packet = UnixResponse {
            error: Some(false),
            tag: None,
            data: Some(data),
        };
        self.write_log(json!({ "push": packet }));
        self.out
            .as_ref()
            .is_some_and(|out| out.send(packet).is_ok())
    }

    fn status(&self) -> UnixResponseData {
        UnixResponseData::ServerStatus(CompetitionStatusResp {
            devices: self.devices.clone(),
            auto_setup: true,
            ..Default::default()
        })
    }

    fn handle(&mut self, req: UnixRequest) -> Option<UnixResponse> {
        let mut dropped = false;
        let reply = match &req.data {
            UnixRequestData::RequestToConnectDevice {
                esp_id, sign_key, ..
            } => {
                // auto-approved; a repeated add replaces the sign key
                self.devices.retain(|d| d.esp_id != *esp_id);
                self.devices.push(CompetitionStatusDevice {
                    esp_id: *esp_id,
                    sign_key: Some(*sign_key),
                });
                let status = self.status();
                self.push(status);
                Ok(UnixResponseData::Empty)
            }
            UnixRequestData::PersonInfo {
                card_id,
                is_competitor,
                ..
            } => self.person_info(card_id, *is_competitor),
            UnixRequestData::EnterAttempt { esp_id, .. } => {
                if let Some(i) = self.drop_replies.iter().position(|e| e == esp_id) {
                    self.drop_replies.remove(i);
                    dropped = true;
                }
                self.enter_attempt(&req.data)
            }
            _ => Ok(UnixResponseData::Empty),
        };

        let resp = UnixResponse {
            error: Some(reply.is_err()),
            tag: req.tag,
            data: Some(reply.unwrap_or_else(|(message, should_reset_time)| {
                UnixResponseData::Error {
                    message: message.to_string(),
                    should_reset_time,
                }
            })),
        };
        let resp = (!dropped && req.tag.is_some()).then_some(resp);
        if !matches!(req.data, UnixRequestData::CurrentTimeInfo { .. }) {
            self.write_log(json!({ "req": req, "resp": resp, "dropped": dropped }));
        }
        resp
    }

    fn effective(&self, competitor: u64, round_id: &str) -> impl Iterator<Item = &Attempt> {
        self.attempts.iter().filter(move |a| {
            a.competitor_id == competitor
                && a.round_id == round_id
                && a.status != Status::ExtraGiven
        })
    }

    fn cutoff_passed(&self, competitor: u64, round_id: &str, cutoff: Option<Cutoff>) -> bool {
        let Some(cutoff) = cutoff else {
            return true;
        };
        let done: Vec<_> = self.effective(competitor, round_id).collect();
        done.len() < cutoff.attempts
            || done.iter().any(|a| {
                a.penalty != -1 && (a.value as i64 + a.penalty * 100) < cutoff.value as i64
            })
    }

    fn round_finished(&self, competitor: u64, round_id: &str) -> bool {
        let info = self.rounds.get(round_id).copied().unwrap_or_default();
        if info.attempts == 0 {
            return false;
        }
        let max = match info.cutoff {
            Some(c) if !self.cutoff_passed(competitor, round_id, info.cutoff) => c.attempts,
            _ => info.attempts as usize,
        };
        self.effective(competitor, round_id).count() >= max
    }

    fn person_info(&self, card_id: &str, is_competitor: bool) -> Reply {
        let Some((card_id, person)) = card_id
            .parse::<u64>()
            .ok()
            .and_then(|id| Some((id, self.tests.cards.get(&id)?)))
        else {
            return Err(("Competitor not found", false));
        };

        let groups: Vec<_> = self
            .tests
            .groups
            .iter()
            .filter(|g| person.groups.contains(&g.group_id))
            .collect();
        let open: Vec<_> = groups
            .iter()
            .filter(|g| !self.round_finished(card_id, round_of(&g.group_id)))
            .map(|g| (*g).clone())
            .collect();
        if is_competitor && !groups.is_empty() && open.is_empty() {
            return Err(("No attempts left", true));
        }

        Ok(UnixResponseData::PersonInfoResp {
            id: card_id.to_string(),
            registrant_id: Some(person.registrant_id),
            name: person.name.clone(),
            wca_id: Some(person.wca_id.clone()),
            country_iso2: Some("PL".to_string()),
            gender: "Male".to_string(),
            can_compete: person.can_compete,
            possible_groups: open,
        })
    }

    fn enter_attempt(&mut self, data: &UnixRequestData) -> Reply {
        let UnixRequestData::EnterAttempt {
            value,
            value_ms,
            penalty,
            esp_id,
            judge_id,
            competitor_id,
            is_delegate,
            session_id,
            inspection_time,
            group_id,
            ..
        } = data
        else {
            return Err(("Wrong request", false));
        };
        let Some(competitor) = competitor_id
            .parse::<u64>()
            .ok()
            .filter(|c| self.tests.cards.contains_key(c))
        else {
            return Err(("Competitor not found", true));
        };
        let judge = judge_id
            .parse::<u64>()
            .ok()
            .filter(|j| self.tests.cards.contains_key(j));

        if let Some(prev) = self
            .attempts
            .iter_mut()
            .find(|a| a.session_id == *session_id && a.esp_id == *esp_id)
        {
            if prev.status != Status::Resolved {
                return Err(("This attempt has already been entered", false));
            }
            // the cards after a resolved incident only add the judge
            let Some(judge) = judge else {
                return Err(("Judge's card was not scanned", false));
            };
            prev.judge_id = Some(judge);
            return ok("Attempt entered");
        }

        if judge.is_none() && !is_delegate {
            return Err(("Judge not found", false));
        }
        if judge == Some(competitor) {
            return Err(("Judge cannot be the same person as competitor", false));
        }
        if !self.tests.groups.iter().any(|g| g.group_id == *group_id) {
            return Err(("Group not found", false));
        }
        let round_id = round_of(group_id).to_string();
        let registered = self.tests.cards[&competitor]
            .groups
            .iter()
            .any(|g| round_of(g) == round_id);
        if !registered {
            return Err(("You are not signed for this event", false));
        }

        let info = self.rounds.get(&round_id).copied().unwrap_or_default();
        if !self.cutoff_passed(competitor, &round_id, info.cutoff) {
            return Err(("Cutoff not passed", true));
        }
        let (penalty, message) = match info.limit {
            Some(limit) if *value >= limit => (-1, "Attempt entered but replaced to DNF"),
            _ => (*penalty, "Attempt entered"),
        };

        let mine = |a: &Attempt| a.competitor_id == competitor && a.round_id == round_id;
        let extras = self.attempts.iter().filter(|a| mine(a) && a.extra).count() as u32;
        let to_replace = self
            .attempts
            .iter_mut()
            .find(|a| mine(a) && a.status == Status::ExtraGiven && a.replaced_by.is_none());
        let (attempt_number, extra) = match to_replace {
            Some(original) => {
                original.replaced_by = Some(extras + 1);
                (extras + 1, true)
            }
            None => {
                let last = self
                    .attempts
                    .iter()
                    .filter(|a| mine(a) && !a.extra)
                    .map(|a| a.attempt_number)
                    .max()
                    .unwrap_or(0);
                if info.attempts > 0 && last >= info.attempts {
                    return Err(("No attempts left", false));
                }
                (last + 1, false)
            }
        };

        self.attempts.push(Attempt {
            session_id: session_id.clone(),
            esp_id: *esp_id,
            competitor_id: competitor,
            judge_id: judge,
            group_id: group_id.clone(),
            round_id,
            attempt_number,
            extra,
            status: if *is_delegate {
                Status::Unresolved
            } else {
                Status::Standard
            },
            value: *value,
            value_ms: *value_ms,
            penalty,
            inspection_time: *inspection_time,
            replaced_by: None,
        });
        if *is_delegate {
            return ok("Delegate was notified");
        }
        ok(message)
    }

    fn command(&mut self, cmd: Command) -> Value {
        let result = match &cmd {
            Command::Attempts => return json!({ "ok": true, "attempts": self.attempts }),
            Command::Resolve {
                session_id,
                status,
                value,
                penalty,
                no_cards,
            } => self.resolve(session_id, *status, *value, *penalty, *no_cards),
            Command::Delete { session_id } => self.delete(session_id),
            Command::DropReply { esp_id } => {
                self.drop_replies.push(*esp_id);
                Ok(true)
            }
        };
        let result = match result {
            Ok(delivered) => json!({ "ok": true, "delivered": delivered }),
            Err(e) => json!({ "ok": false, "error": e }),
        };
        self.write_log(json!({ "ctl": cmd, "result": result }));
        result
    }

    /// The delegate decides on the panel (FKMTime `updateAttempt`); returns whether the
    /// `IncidentResolved` push reached a micro-connector.
    fn resolve(
        &mut self,
        session_id: &str,
        status: Status,
        value: Option<u64>,
        penalty: Option<i64>,
        no_cards: bool,
    ) -> Result<bool, &'static str> {
        if status == Status::Unresolved {
            return Err("an incident is resolved to another status");
        }
        let attempt = self
            .attempts
            .iter_mut()
            .find(|a| a.session_id == session_id && a.status == Status::Unresolved)
            .ok_or("no unresolved attempt with this session id")?;
        attempt.status = status;
        attempt.value = value.unwrap_or(attempt.value);
        attempt.penalty = penalty.unwrap_or(attempt.penalty);
        let packet = UnixResponseData::IncidentResolved {
            esp_id: attempt.esp_id,
            should_scan_cards: !no_cards && status == Status::Resolved,
            attempt: IncidentAttempt {
                session_id: attempt.session_id.clone(),
                penalty: Some(attempt.penalty),
                value: Some(attempt.value),
            },
        };
        Ok(self.push(packet))
    }

    /// The delegate deletes the attempt (FKMTime `deleteAttempt`).
    fn delete(&mut self, session_id: &str) -> Result<bool, &'static str> {
        let idx = self
            .attempts
            .iter()
            .position(|a| a.session_id == session_id)
            .ok_or("no attempt with this session id")?;
        let attempt = self.attempts.remove(idx);
        if attempt.status != Status::Unresolved {
            return Ok(false);
        }
        Ok(self.push(UnixResponseData::IncidentResolved {
            esp_id: attempt.esp_id,
            should_scan_cards: false,
            attempt: IncidentAttempt {
                session_id: attempt.session_id,
                penalty: Some(attempt.penalty),
                value: Some(attempt.value),
            },
        }))
    }
}

pub async fn run(socket_path: &str, tests_json: &[u8]) -> Result<()> {
    let tests: TestsRoot = serde_json::from_slice(tests_json)?;
    let config: SimConfig = serde_json::from_slice(tests_json)?;
    let log_path = std::env::var("E2E_LOG").unwrap_or("e2e.jsonl".to_string());
    let control_path = std::env::var("E2E_CONTROL").unwrap_or(format!("{socket_path}.ctl"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let fkm = Arc::new(Mutex::new(Fkm {
        tests,
        rounds: config.rounds,
        devices: Vec::new(),
        attempts: Vec::new(),
        drop_replies: Vec::new(),
        log,
        out: None,
    }));

    _ = tokio::fs::remove_file(&control_path).await;
    let control = UnixListener::bind(&control_path)?;
    tokio::spawn(control_task(control, fkm.clone()));

    let listener = UnixListener::bind(socket_path)?;
    tracing::info!("Sim mode: socket {socket_path}, control {control_path}, log {log_path}");
    loop {
        let (stream, _) = listener.accept().await?;
        if let Err(e) = serve(stream, &fkm).await {
            tracing::error!("Connection ended: {e:?}");
        }
        lock(&fkm).out = None;
    }
}

fn lock(fkm: &Mutex<Fkm>) -> std::sync::MutexGuard<'_, Fkm> {
    // a panic while holding the lock can't leave the model half-updated in a way that matters
    fkm.lock().unwrap_or_else(|e| e.into_inner())
}

async fn serve(stream: UnixStream, fkm: &Mutex<Fkm>) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = unbounded_channel::<UnixResponse>();
    let writer = tokio::spawn(async move {
        while let Some(packet) = rx.recv().await {
            let Ok(mut bytes) = serde_json::to_vec(&packet) else {
                continue;
            };
            bytes.push(0x00);
            if write.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });

    {
        let mut fkm = lock(fkm);
        fkm.out = Some(tx.clone());
        let status = fkm.status();
        fkm.push(status);
    }

    let mut read = BufReader::new(read);
    let mut buf = Vec::with_capacity(512);
    let res = loop {
        buf.clear();
        match read.read_until(0x00, &mut buf).await {
            Ok(0) => break Ok(()),
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
        if buf.last() == Some(&0x00) {
            buf.pop();
        }
        let req: UnixRequest = match serde_json::from_slice(&buf) {
            Ok(req) => req,
            Err(e) => {
                tracing::error!("Unparsable request: {e:?}");
                continue;
            }
        };
        if let Some(resp) = lock(fkm).handle(req) {
            _ = tx.send(resp);
        }
    };
    writer.abort();
    res
}

async fn control_task(listener: UnixListener, fkm: Arc<Mutex<Fkm>>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let fkm = fkm.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let reply = match serde_json::from_str::<Command>(&line) {
                    Ok(cmd) => lock(&fkm).command(cmd),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                };
                if write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
}
