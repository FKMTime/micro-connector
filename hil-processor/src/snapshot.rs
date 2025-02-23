use crate::{HilDevice, HilState};
pub use unix_utils::SnapshotData;

pub fn snapshot_dsl_check(
    hil_state: &HilState,
    device: &HilDevice,
    snapshot: &SnapshotData,
    check: &str,
) -> Result<bool, ()> {
    let dsl_split = check.split(" ").collect::<Vec<&str>>();
    if dsl_split.len() != 3 {
        crate::error!(hil_state, "Wrong dsl line! {check}");
        return Err(());
    }

    let check_op = match dsl_split[1] {
        ">" => DslOp::GreaterThan(dsl_split[2].parse().map_err(|_| ())?),
        "<" => DslOp::SmallerThan(dsl_split[2].parse().map_err(|_| ())?),
        "==" => DslOp::Equal({
            match dsl_split[2] {
                "true" => 1,
                "false" => 0,
                "timer" => device.last_solve_time as i128,
                _ => dsl_split[2].parse().map_err(|_| ())?,
            }
        }),
        "!=" => DslOp::NotEqual({
            match dsl_split[2] {
                "true" => 1,
                "false" => 0,
                "timer" => device.last_solve_time as i128,
                _ => dsl_split[2].parse().map_err(|_| ())?,
            }
        }),
        "is" => DslOp::Is(match dsl_split[2] {
            "some" => true,
            "none" => false,
            _ => {
                crate::error!(hil_state, "Wrong dsl IS op! {check}");
                return Err(());
            }
        }),
        _ => DslOp::SmallerThan(0),
    };

    let value_to_check: Option<i128> = match dsl_split[0] {
        "scene" => Some(snapshot.scene as i128),
        "inspection_time" => snapshot.inspection_time.map(|t| t as i128),
        "solve_time" => snapshot.solve_time.map(|t| t as i128),
        "penalty" => Some(snapshot.penalty.unwrap_or(0) as i128),
        "time_confirmed" => Some(snapshot.time_confirmed as i128),
        "possible_groups" => Some(snapshot.possible_groups as i128),
        "group_selected_idx" => Some(snapshot.group_selected_idx as i128),
        "current_competitor" => snapshot.current_competitor.map(|c| c as i128),
        "current_judge" => snapshot.current_judge.map(|c| c as i128),
        _ => {
            crate::error!(hil_state, "Dsl not implemented! {check}");
            return Err(());
        }
    };

    return Ok(check_op.check_against(value_to_check));
}

enum DslOp {
    SmallerThan(i128),
    GreaterThan(i128),
    Equal(i128),
    NotEqual(i128),
    Is(bool),
}

impl DslOp {
    pub fn check_against(&self, value: Option<i128>) -> bool {
        match self {
            DslOp::SmallerThan(b) => {
                let Some(value) = value else {
                    return false;
                };

                return value < *b;
            }
            DslOp::GreaterThan(b) => {
                let Some(value) = value else {
                    return false;
                };

                return value > *b;
            }
            DslOp::Equal(b) => {
                let Some(value) = value else {
                    return false;
                };

                return value == *b;
            }
            DslOp::NotEqual(b) => {
                let Some(value) = value else {
                    return false;
                };

                return value != *b;
            }
            DslOp::Is(some) => {
                return *some == value.is_some();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn generate_state() -> (HilState, HilDevice, SnapshotData) {
        let state = HilState {
            tests: crate::structs::TestsRoot {
                dump_state_after_test: false,
                groups: Vec::new(),
                cards: HashMap::new(),
                buttons: HashMap::new(),
                tests: Vec::new(),
            },
            status: unix_utils::response::CompetitionStatusResp {
                should_update: false,
                devices: Vec::new(),
                translations: Vec::new(),
                default_locale: "".to_string(),
            },
            get_ms: || 0,
            devices: Vec::new(),
            log_fn: |tag, msg| println!("[{tag}] {msg}"),
            packet_queue: Vec::new(),
            completed_count: 0,
            should_send_status: false,
        };

        let device = HilDevice {
            id: 0,
            back_packet: None,
            completed_count: 0,
            last_solve_time: 69420,
            remove_after: false,
            last_test: 0,
            current_test: None,
            current_step: 0,
            wait_for_ack: false,
            next_step_time: 0,
        };

        let snapshot = SnapshotData {
            scene: 0,
            inspection_time: None,
            solve_time: None,
            penalty: None,
            time_confirmed: false,
            possible_groups: 0,
            group_selected_idx: 0,
            current_competitor: None,
            current_judge: None,
        };

        (state, device, snapshot)
    }

    #[test]
    fn test_smaller_than() {
        let (state, device, mut snapshot) = generate_state();
        snapshot.possible_groups = 1;

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "possible_groups < 2"),
            Ok(true)
        );
        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "possible_groups < 1"),
            Ok(false)
        );
    }

    #[test]
    fn test_greater_than() {
        let (state, device, mut snapshot) = generate_state();
        snapshot.group_selected_idx = 3;

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "group_selected_idx > 2"),
            Ok(true)
        );
        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "group_selected_idx > 3"),
            Ok(false)
        );
    }

    #[test]
    fn test_equal() {
        let (state, device, mut snapshot) = generate_state();
        snapshot.time_confirmed = true;
        snapshot.solve_time = Some(69420); // same as device last solve time

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "time_confirmed == true"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "time_confirmed == 1"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "time_confirmed == false"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time == 69420"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time == 12345"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time == timer"),
            Ok(true)
        );

        snapshot.solve_time = Some(12345);

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time == timer"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "penalty == 0"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "penalty == 1"),
            Ok(false)
        );
    }

    #[test]
    fn test_notequal() {
        let (state, device, mut snapshot) = generate_state();
        snapshot.time_confirmed = true;
        snapshot.solve_time = Some(69420); // same as device last solve time

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "time_confirmed != true"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "time_confirmed != 1"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "time_confirmed != false"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time != 69420"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time != 12345"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time != timer"),
            Ok(false)
        );

        snapshot.solve_time = Some(12345);

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time != timer"),
            Ok(true)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "penalty != 0"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "penalty != 1"),
            Ok(true)
        );
    }

    #[test]
    fn test_is() {
        let (state, device, mut snapshot) = generate_state();
        snapshot.inspection_time = Some(123);

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "inspection_time is some"),
            Ok(true)
        );
        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "inspection_time is none"),
            Ok(false)
        );

        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time is some"),
            Ok(false)
        );
        assert_eq!(
            snapshot_dsl_check(&state, &device, &snapshot, "solve_time is none"),
            Ok(true)
        );
    }
}
