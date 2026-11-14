use anyhow::{Result, bail};

/// v3.4.x: 'L' + truncated flag + u64 time (0 = none)
const HEADER_LEN_TIME_ONLY: usize = 10;
/// Untagged builds before v3.5.0: 'L' + truncated flag + u64 time + u64 inspection
/// + u64 competitor + 20 B group id + padding (u64::MAX / 0x00 = none)
const HEADER_LEN_NO_SESSION: usize = 50;
/// v3.5.0: same as above + 36 B session id + padding
const HEADER_LEN: usize = 100;
/// Live-state block inside the padding of the v3.5.0 header: magic, version, flags.
/// Older firmware leaves these bytes uninitialised, so they only count behind the magic.
const LIVE_STATE_OFFSET: usize = 82;
const LIVE_STATE_MAGIC: &[u8; 2] = b"LV";
const LIVE_STATE_VERSION: u8 = 1;
const LIVE_FLAG_TIMER_RUNNING: u8 = 1 << 0;
const LIVE_FLAG_INSPECTION_RUNNING: u8 = 1 << 1;
/// Station's Wi-Fi RSSI (i8, dBm) right after the live-state block; `0` = no reading.
/// Firmware before it zero-fills the byte, so it reads as unknown without a version bump.
const LIVE_RSSI_OFFSET: usize = LIVE_STATE_OFFSET + 4;

#[derive(Debug, Default, PartialEq)]
pub struct LogPacket<'a> {
    pub truncated: bool,
    pub current_time: Option<u64>,
    pub inspection_time: Option<u64>,
    pub current_competitor: Option<u64>,
    pub current_group_id: Option<String>,
    pub current_session_id: Option<String>,
    /// `None` when the firmware doesn't report it (no live-state block in the header).
    pub timer_running: Option<bool>,
    pub inspection_running: Option<bool>,
    /// Station's Wi-Fi signal in dBm; `None` without the live-state block or a reading.
    pub rssi: Option<i8>,
    pub lines: Vec<&'a str>,
    /// A line ran past the end of the packet or wasn't utf8. `lines` stops before it.
    pub lines_malformed: bool,
}

/// The header carries no version, but every batch starts with a line and every line with its
/// level ("I "). Those bytes can't appear where a header is read instead: at 12 they would be
/// the top of a u64 ms inspection time, at 52 the middle of a uuid or zeros.
fn line_starts_at(buf: &[u8], offset: usize) -> bool {
    matches!(
        buf.get(offset + 2..offset + 4),
        Some([b'E' | b'W' | b'I' | b'D' | b'T', b' '])
    )
}

fn read_u64(buf: &[u8], offset: usize, none_value: u64) -> Option<u64> {
    let bytes = buf.get(offset..offset + 8)?.try_into().ok()?;
    Some(u64::from_be_bytes(bytes)).filter(|&value| value != none_value)
}

fn read_live_flags(buf: &[u8]) -> Option<u8> {
    let block = buf.get(LIVE_STATE_OFFSET..LIVE_STATE_OFFSET + 4)?;
    (block[..2] == *LIVE_STATE_MAGIC && block[2] == LIVE_STATE_VERSION).then_some(block[3])
}

fn read_rssi(buf: &[u8]) -> Option<i8> {
    buf.get(LIVE_RSSI_OFFSET)
        .map(|&byte| byte as i8)
        .filter(|&rssi| rssi != 0)
}

fn read_str(buf: &[u8], offset: usize, len: usize) -> Option<String> {
    let raw = buf.get(offset..offset + len)?;
    let end = raw.iter().position(|&b| b == 0x00).unwrap_or(raw.len());
    if end == 0 {
        None
    } else {
        String::from_utf8(raw[..end].to_vec()).ok()
    }
}

pub fn parse_log_packet(buf: &[u8]) -> Result<LogPacket<'_>> {
    let header_len = if line_starts_at(buf, HEADER_LEN_TIME_ONLY) {
        HEADER_LEN_TIME_ONLY
    } else if line_starts_at(buf, HEADER_LEN_NO_SESSION) {
        HEADER_LEN_NO_SESSION
    } else {
        HEADER_LEN
    };

    if buf.len() < header_len {
        bail!("Packet too short ({} bytes)", buf.len());
    }

    let mut packet = LogPacket {
        truncated: buf[1] == 0x01,
        ..Default::default()
    };

    if header_len == HEADER_LEN_TIME_ONLY {
        packet.current_time = read_u64(buf, 2, 0);
    } else {
        packet.current_time = read_u64(buf, 2, u64::MAX);
        packet.inspection_time = read_u64(buf, 10, u64::MAX);
        packet.current_competitor = read_u64(buf, 18, u64::MAX);
        packet.current_group_id = read_str(buf, 26, 20);
    }

    if header_len == HEADER_LEN {
        packet.current_session_id = read_str(buf, 46, 36);

        if let Some(flags) = read_live_flags(buf) {
            packet.timer_running = Some(flags & LIVE_FLAG_TIMER_RUNNING != 0);
            packet.inspection_running = Some(flags & LIVE_FLAG_INSPECTION_RUNNING != 0);
            packet.rssi = read_rssi(buf);
        }
    }

    let mut offset = header_len;
    while offset < buf.len() {
        let line = buf
            .get(offset..offset + 2)
            .map(|len| offset + 2 + u16::from_be_bytes([len[0], len[1]]) as usize)
            .and_then(|end| buf.get(offset + 2..end))
            .and_then(|line| core::str::from_utf8(line).ok());

        let Some(line) = line else {
            packet.lines_malformed = true;
            break;
        };

        if !line.is_empty() {
            packet.lines.push(line);
        }

        offset += 2 + line.len();
    }

    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIME: u64 = 12_345;
    const INSPECTION: u64 = 8_000;
    const COMPETITOR: u64 = 0xA1B2_C3D4;
    const GROUP_ID: &str = "333-r1-a";
    const SESSION_ID: &str = "00000000-1111-4222-8333-444444444444";

    fn with_line(mut packet: Vec<u8>, line: &str) -> Vec<u8> {
        packet.extend_from_slice(&(line.len() as u16).to_be_bytes());
        packet.extend_from_slice(line.as_bytes());
        packet
    }

    fn time_only_header(time: u64) -> Vec<u8> {
        let mut packet = vec![b'L', 0x00];
        packet.extend_from_slice(&time.to_be_bytes());
        packet
    }

    fn header(len: usize, time: u64, inspection: u64, competitor: u64) -> Vec<u8> {
        let mut packet = vec![0; len];
        packet[0] = b'L';
        packet[2..10].copy_from_slice(&time.to_be_bytes());
        packet[10..18].copy_from_slice(&inspection.to_be_bytes());
        packet[18..26].copy_from_slice(&competitor.to_be_bytes());
        packet[26..26 + GROUP_ID.len()].copy_from_slice(GROUP_ID.as_bytes());
        packet
    }

    fn current_header() -> Vec<u8> {
        let mut packet = header(HEADER_LEN, TIME, INSPECTION, COMPETITOR);
        packet[46..46 + SESSION_ID.len()].copy_from_slice(SESSION_ID.as_bytes());
        packet
    }

    fn with_live_state(mut packet: Vec<u8>, version: u8, flags: u8) -> Vec<u8> {
        packet[LIVE_STATE_OFFSET..LIVE_STATE_OFFSET + 2].copy_from_slice(LIVE_STATE_MAGIC);
        packet[LIVE_STATE_OFFSET + 2] = version;
        packet[LIVE_STATE_OFFSET + 3] = flags;
        packet
    }

    /// Sized like the packets that used to panic: long enough to pass the old length check,
    /// too short to reach the end of the session id.
    #[test]
    fn time_only_packet_of_51_to_81_bytes() {
        for total_len in [51, 56, 72, 80, 81] {
            let line = format!("I {}", "x".repeat(total_len - HEADER_LEN_TIME_ONLY - 4));
            let packet = with_line(time_only_header(TIME), &line);
            assert_eq!(packet.len(), total_len);

            assert_eq!(
                parse_log_packet(&packet).unwrap(),
                LogPacket {
                    current_time: Some(TIME),
                    lines: vec![line.as_str()],
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn time_only_packet_long_enough_to_look_like_current() {
        let long_line = format!("W {}", "x".repeat(120));
        let packet = with_line(time_only_header(TIME), "I first");
        let packet = with_line(packet, &long_line);

        assert_eq!(
            parse_log_packet(&packet).unwrap(),
            LogPacket {
                current_time: Some(TIME),
                lines: vec!["I first", long_line.as_str()],
                ..Default::default()
            }
        );
    }

    #[test]
    fn time_only_packet_treats_zero_as_no_time() {
        let packet = with_line(time_only_header(0), "D idle");
        let parsed = parse_log_packet(&packet).unwrap();

        assert_eq!(parsed.current_time, None);
        assert_eq!(parsed.lines, ["D idle"]);
    }

    #[test]
    fn no_session_packet() {
        let packet = with_line(
            header(HEADER_LEN_NO_SESSION, TIME, INSPECTION, COMPETITOR),
            "E failed",
        );

        assert_eq!(
            parse_log_packet(&packet).unwrap(),
            LogPacket {
                current_time: Some(TIME),
                inspection_time: Some(INSPECTION),
                current_competitor: Some(COMPETITOR),
                current_group_id: Some(GROUP_ID.to_string()),
                lines: vec!["E failed"],
                ..Default::default()
            }
        );
    }

    #[test]
    fn current_packet() {
        let packet = with_line(with_line(current_header(), "I one"), "T two");

        assert_eq!(
            parse_log_packet(&packet).unwrap(),
            LogPacket {
                current_time: Some(TIME),
                inspection_time: Some(INSPECTION),
                current_competitor: Some(COMPETITOR),
                current_group_id: Some(GROUP_ID.to_string()),
                current_session_id: Some(SESSION_ID.to_string()),
                lines: vec!["I one", "T two"],
                ..Default::default()
            }
        );
    }

    #[test]
    fn current_packet_with_nothing_live() {
        let mut packet = header(HEADER_LEN, u64::MAX, u64::MAX, u64::MAX);
        packet[26..82].fill(0x00);
        let packet = with_line(packet, "I idle");
        let parsed = parse_log_packet(&packet).unwrap();

        assert_eq!(parsed.current_time, None);
        assert_eq!(parsed.inspection_time, None);
        assert_eq!(parsed.current_competitor, None);
        assert_eq!(parsed.current_group_id, None);
        assert_eq!(parsed.current_session_id, None);
    }

    #[test]
    fn truncated_flag() {
        let mut packet = with_line(current_header(), "I one");
        packet[1] = 0x01;

        assert!(parse_log_packet(&packet).unwrap().truncated);
    }

    #[test]
    fn packet_too_short_for_any_layout() {
        assert!(parse_log_packet(b"").is_err());
        assert!(parse_log_packet(b"L").is_err());
        assert!(parse_log_packet(&time_only_header(TIME)).is_err());
        assert!(parse_log_packet(&current_header()[..HEADER_LEN - 1]).is_err());
    }

    #[test]
    fn line_longer_than_packet_keeps_earlier_lines() {
        let mut packet = with_line(current_header(), "I kept");
        packet.extend_from_slice(&50u16.to_be_bytes());
        packet.extend_from_slice(b"I cut sh");

        let parsed = parse_log_packet(&packet).unwrap();
        assert_eq!(parsed.lines, ["I kept"]);
        assert!(parsed.lines_malformed);
    }

    #[test]
    fn line_ending_two_bytes_past_packet() {
        let mut packet = current_header();
        packet.extend_from_slice(&4u16.to_be_bytes());
        packet.extend_from_slice(b"I x");

        let parsed = parse_log_packet(&packet).unwrap();
        assert!(parsed.lines.is_empty());
        assert!(parsed.lines_malformed);
    }

    #[test]
    fn line_length_cut_in_half() {
        let mut packet = with_line(time_only_header(TIME), "I kept");
        packet.push(0x00);

        let parsed = parse_log_packet(&packet).unwrap();
        assert_eq!(parsed.lines, ["I kept"]);
        assert!(parsed.lines_malformed);
    }

    #[test]
    fn line_with_invalid_utf8() {
        let mut packet = with_line(current_header(), "I kept");
        packet.extend_from_slice(&3u16.to_be_bytes());
        packet.extend_from_slice(&[b'I', b' ', 0xFF]);

        let parsed = parse_log_packet(&packet).unwrap();
        assert_eq!(parsed.lines, ["I kept"]);
        assert!(parsed.lines_malformed);
    }

    #[test]
    fn empty_line_is_skipped() {
        let packet = with_line(with_line(current_header(), ""), "I after");
        let parsed = parse_log_packet(&packet).unwrap();

        assert_eq!(parsed.lines, ["I after"]);
        assert!(!parsed.lines_malformed);
    }

    #[test]
    fn any_prefix_of_any_layout_is_safe() {
        let packets = [
            with_line(with_line(time_only_header(TIME), "I one"), "W two"),
            with_line(
                header(HEADER_LEN_NO_SESSION, TIME, INSPECTION, COMPETITOR),
                "I one",
            ),
            with_line(with_line(current_header(), "I one"), "W two"),
            with_live_state(current_header(), LIVE_STATE_VERSION, 0b11),
        ];

        for packet in &packets {
            for len in 0..=packet.len() {
                _ = parse_log_packet(&packet[..len]);
            }
        }
    }

    #[test]
    fn live_state_flags() {
        let cases = [
            (0b00, false, false),
            (0b01, true, false),
            (0b10, false, true),
            (0b11, true, true),
        ];

        for (flags, timer_running, inspection_running) in cases {
            let packet = with_live_state(current_header(), LIVE_STATE_VERSION, flags);
            let packet = with_line(packet, "I one");

            assert_eq!(
                parse_log_packet(&packet).unwrap(),
                LogPacket {
                    current_time: Some(TIME),
                    inspection_time: Some(INSPECTION),
                    current_competitor: Some(COMPETITOR),
                    current_group_id: Some(GROUP_ID.to_string()),
                    current_session_id: Some(SESSION_ID.to_string()),
                    timer_running: Some(timer_running),
                    inspection_running: Some(inspection_running),
                    lines: vec!["I one"],
                    ..Default::default()
                },
                "flags {flags:#04b}"
            );
        }
    }

    #[test]
    fn live_state_ignores_unknown_flag_bits() {
        let packet = with_live_state(current_header(), LIVE_STATE_VERSION, 0b1111_1101);
        let parsed = parse_log_packet(&packet).unwrap();

        assert_eq!(parsed.timer_running, Some(true));
        assert_eq!(parsed.inspection_running, Some(false));
    }

    /// Firmware before the live-state block leaves 82..100 uninitialised.
    #[test]
    fn live_state_is_none_without_the_block() {
        let mut garbage = current_header();
        garbage[LIVE_STATE_OFFSET..HEADER_LEN].fill(0xA5);
        let mut lowercase_magic = with_live_state(current_header(), LIVE_STATE_VERSION, 0b11);
        lowercase_magic[LIVE_STATE_OFFSET + 1] = b'v';

        let packets = [
            current_header(),
            garbage,
            with_live_state(current_header(), 0, 0b11),
            with_live_state(current_header(), LIVE_STATE_VERSION + 1, 0b11),
            lowercase_magic,
        ];

        for packet in packets {
            let packet = with_line(packet, "I one");
            let parsed = parse_log_packet(&packet).unwrap();

            assert_eq!(parsed.timer_running, None);
            assert_eq!(parsed.inspection_running, None);
            assert_eq!(parsed.current_session_id.as_deref(), Some(SESSION_ID));
            assert_eq!(parsed.lines, ["I one"]);
        }
    }

    fn with_rssi(mut packet: Vec<u8>, rssi: i8) -> Vec<u8> {
        packet[LIVE_RSSI_OFFSET] = rssi as u8;
        packet
    }

    #[test]
    fn rssi_from_the_live_header() {
        let packet = with_rssi(
            with_live_state(current_header(), LIVE_STATE_VERSION, 0b01),
            -67,
        );
        let packet = with_line(packet, "I one");
        let parsed = parse_log_packet(&packet).unwrap();

        assert_eq!(parsed.rssi, Some(-67));
        assert_eq!(parsed.timer_running, Some(true));
        assert_eq!(parsed.lines, ["I one"]);
    }

    #[test]
    fn rssi_zero_is_no_reading() {
        let packet = with_rssi(
            with_live_state(current_header(), LIVE_STATE_VERSION, 0b00),
            0,
        );

        assert_eq!(parse_log_packet(&packet).unwrap().rssi, None);
    }

    /// Byte 86 means nothing unless the live-state block in front of it is valid.
    #[test]
    fn rssi_is_none_without_the_live_block() {
        let packets = [
            with_rssi(current_header(), -67),
            with_rssi(with_live_state(current_header(), 0, 0b11), -67),
            with_rssi(
                with_live_state(current_header(), LIVE_STATE_VERSION + 1, 0b11),
                -67,
            ),
        ];

        for packet in packets {
            assert_eq!(parse_log_packet(&packet).unwrap().rssi, None);
        }
    }

    /// Sent every second and on timer start/stop: live state only, no lines.
    #[test]
    fn header_only_packet() {
        let packet = with_live_state(current_header(), LIVE_STATE_VERSION, 0b01);
        assert_eq!(packet.len(), HEADER_LEN);

        assert_eq!(
            parse_log_packet(&packet).unwrap(),
            LogPacket {
                current_time: Some(TIME),
                inspection_time: Some(INSPECTION),
                current_competitor: Some(COMPETITOR),
                current_group_id: Some(GROUP_ID.to_string()),
                current_session_id: Some(SESSION_ID.to_string()),
                timer_running: Some(true),
                inspection_running: Some(false),
                rssi: None,
                lines: vec![],
                lines_malformed: false,
                truncated: false,
            }
        );
    }

    #[test]
    fn header_only_packet_with_nothing_live() {
        let mut packet = header(HEADER_LEN, u64::MAX, u64::MAX, u64::MAX);
        packet[26..82].fill(0x00);
        let packet = with_live_state(packet, LIVE_STATE_VERSION, 0b00);

        assert_eq!(
            parse_log_packet(&packet).unwrap(),
            LogPacket {
                timer_running: Some(false),
                inspection_running: Some(false),
                ..Default::default()
            }
        );
    }
}
