use anyhow::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorLogEntry {
    Code {
        timestamp: u64,
        code: u8,
    },
    Stacktrace {
        timestamp: u64,
        version: String,
        addrs: Vec<u32>,
    },
}

pub fn parse_error_log_entries(error_log_buf: &[u8]) -> Result<Vec<ErrorLogEntry>> {
    let mut tmp = Vec::new();

    let mut offset = 0;
    while offset < error_log_buf.len() {
        let remaining = &error_log_buf[offset..];
        let log_type = remaining[0];
        match log_type {
            b'N' => {
                // u64 + u8
                if remaining.len() < 1 + 8 + 1 {
                    break;
                }

                let entry = ErrorLogEntry::Code {
                    timestamp: u64::from_be_bytes(remaining[1..1 + 8].try_into()?),
                    code: remaining[1 + 8],
                };

                tmp.push(entry);
                offset += 1 + 8 + 1;
            }
            b'S' => {
                // u64 + 16 * u8 + u8 (size) + size * u32
                if remaining.len() < 1 + 8 + 16 + 1 {
                    break;
                }

                let size = remaining[1 + 8 + 16];
                if remaining.len() < 1 + 8 + 16 + 1 + size as usize * 4 {
                    break;
                }

                let version_str = core::str::from_utf8(&remaining[1 + 8..1 + 8 + 16])?;
                let version_str = version_str.trim_end_matches('\0');

                let mut tmp_addrs = Vec::new();
                for addr in remaining[1 + 8 + 16 + 1..1 + 8 + 16 + 1 + size as usize * 4].chunks(4)
                {
                    tmp_addrs.push(u32::from_be_bytes(addr.try_into()?));
                }
                let entry = ErrorLogEntry::Stacktrace {
                    timestamp: u64::from_be_bytes(remaining[1..1 + 8].try_into()?),
                    version: version_str.to_string(),
                    addrs: tmp_addrs,
                };

                tmp.push(entry);
                offset += 1 + 8 + 16 + 1 + size as usize * 4;
            }
            _ => {
                break;
            }
        }
    }

    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::{ErrorLogEntry, parse_error_log_entries};

    fn code_entry(ts: u64, code: u8) -> Vec<u8> {
        let mut buf = vec![b'N'];
        buf.extend_from_slice(&ts.to_be_bytes());
        buf.push(code);
        buf
    }

    fn stack_entry(ts: u64, version: &[u8; 16], addrs: &[u32]) -> Vec<u8> {
        let mut buf = vec![b'S'];
        buf.extend_from_slice(&ts.to_be_bytes());
        buf.extend_from_slice(version);
        buf.push(addrs.len() as u8);
        for a in addrs {
            buf.extend_from_slice(&a.to_be_bytes());
        }
        buf
    }

    #[test]
    fn parses_valid_entries() {
        let mut buf = code_entry(123, 5);
        buf.extend(stack_entry(456, b"v3.4.4\0\0\0\0\0\0\0\0\0\0", &[0xDEAD, 0xBEEF]));

        let parsed = parse_error_log_entries(&buf).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            ErrorLogEntry::Code {
                timestamp: 123,
                code: 5
            }
        );
        match &parsed[1] {
            ErrorLogEntry::Stacktrace {
                timestamp,
                version,
                addrs,
            } => {
                assert_eq!(*timestamp, 456);
                assert_eq!(version, "v3.4.4");
                assert_eq!(addrs, &[0xDEAD, 0xBEEF]);
            }
            _ => panic!("wrong entry type"),
        }
    }

    #[test]
    fn truncated_buffers_do_not_panic() {
        let mut full = code_entry(1, 2);
        full.extend(stack_entry(3, b"abc\0\0\0\0\0\0\0\0\0\0\0\0\0", &[1, 2, 3]));

        // every possible truncation must parse without panic
        for len in 0..full.len() {
            _ = parse_error_log_entries(&full[..len]).unwrap();
        }
    }

    #[test]
    fn unknown_type_stops_parsing() {
        let mut buf = vec![b'X', 1, 2, 3];
        buf.extend(code_entry(7, 8));
        let parsed = parse_error_log_entries(&buf).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn random_buffers_do_not_panic() {
        // simple xorshift, no external deps
        let mut state = 0x9E3779B97F4A7C15u64;
        for _ in 0..1000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = (state % 96) as usize;
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                buf.push(state as u8);
            }
            _ = parse_error_log_entries(&buf); // must not panic
        }
    }
}
