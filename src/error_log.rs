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
    let read = |start: usize, end: usize| {
        error_log_buf
            .get(start..end)
            .ok_or_else(|| anyhow::anyhow!("Error log entry truncated"))
    };

    let mut offset = 0;
    while offset < error_log_buf.len() {
        let log_type = error_log_buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                let entry = ErrorLogEntry::Code {
                    timestamp: u64::from_be_bytes(read(offset + 1, offset + 1 + 8)?.try_into()?),
                    code: read(offset + 1 + 8, offset + 1 + 8 + 1)?[0],
                };

                tmp.push(entry);
                offset += 1 + 8 + 1;
            }
            b'S' => {
                // u64 + 16 * u8 + u8 (size) + size * u32

                let version_str = core::str::from_utf8(read(offset + 1 + 8, offset + 1 + 8 + 16)?)?;
                let version_str = version_str.trim_end_matches('\0');

                let size = read(offset + 1 + 8 + 16, offset + 1 + 8 + 16 + 1)?[0];
                let mut tmp_addrs = Vec::new();
                for addr in read(
                    offset + 1 + 8 + 16 + 1,
                    offset + 1 + 8 + 16 + 1 + size as usize * 4,
                )?
                .chunks(4)
                {
                    tmp_addrs.push(u32::from_be_bytes(addr.try_into()?));
                }
                let entry = ErrorLogEntry::Stacktrace {
                    timestamp: u64::from_be_bytes(read(offset + 1, offset + 1 + 8)?.try_into()?),
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
    use super::*;

    fn code_entry() -> Vec<u8> {
        let mut entry = vec![b'N'];
        entry.extend_from_slice(&1_000u64.to_be_bytes());
        entry.push(7);
        entry
    }

    fn stacktrace_entry() -> Vec<u8> {
        let mut entry = vec![b'S'];
        entry.extend_from_slice(&2_000u64.to_be_bytes());
        entry.extend_from_slice(b"v0.0.0\0\0\0\0\0\0\0\0\0\0");
        entry.push(2);
        entry.extend_from_slice(&0x4200_0000u32.to_be_bytes());
        entry.extend_from_slice(&0x4200_0004u32.to_be_bytes());
        entry
    }

    #[test]
    fn parses_entries() {
        let buf = [code_entry(), stacktrace_entry()].concat();

        assert_eq!(
            parse_error_log_entries(&buf).unwrap(),
            [
                ErrorLogEntry::Code {
                    timestamp: 1_000,
                    code: 7,
                },
                ErrorLogEntry::Stacktrace {
                    timestamp: 2_000,
                    version: "v0.0.0".to_string(),
                    addrs: vec![0x4200_0000, 0x4200_0004],
                },
            ]
        );
    }

    #[test]
    fn truncated_entry_is_an_error() {
        for entry in [code_entry(), stacktrace_entry()] {
            for len in 1..entry.len() {
                assert!(parse_error_log_entries(&entry[..len]).is_err());
            }
        }
    }
}
