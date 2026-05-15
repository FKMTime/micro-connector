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
        let log_type = error_log_buf[offset];
        match log_type {
            b'N' => {
                // u64 + u8
                let entry = ErrorLogEntry::Code {
                    timestamp: u64::from_be_bytes(
                        error_log_buf[offset + 1..offset + 1 + 8].try_into()?,
                    ),
                    code: error_log_buf[offset + 1 + 8],
                };

                tmp.push(entry);
                offset += 1 + 8 + 1;
            }
            b'S' => {
                // u64 + 16 * u8 + u8 (size) + size * u32

                let version_str =
                    core::str::from_utf8(&error_log_buf[offset + 1 + 8..offset + 1 + 8 + 16])?;
                let version_str = version_str.trim_end_matches('\0');

                let size = error_log_buf[offset + 1 + 8 + 16];
                let mut tmp_addrs = Vec::new();
                for addr in (error_log_buf
                    [offset + 1 + 8 + 16 + 1..offset + 1 + 8 + 16 + 1 + size as usize * 4])
                    .chunks(4)
                {
                    tmp_addrs.push(u32::from_be_bytes(addr.try_into()?));
                }
                let entry = ErrorLogEntry::Stacktrace {
                    timestamp: u64::from_be_bytes(
                        error_log_buf[offset + 1..offset + 1 + 8].try_into()?,
                    ),
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
