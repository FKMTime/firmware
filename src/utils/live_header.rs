//! Header of a log batch: the station's live state, sent every interval (no deps, host-tested).
//!
//! `L`, truncated flag, u64 BE time, inspection and competitor (`u64::MAX` = none),
//! 20 B group id, 36 B session id, `LV` marker, version, flags, Wi-Fi RSSI (i8 dBm, `0` = unknown)
//! at 86, zero padding to [`HEADER_LEN`].

pub const HEADER_LEN: usize = 100;
/// At 82..84: says version and flags follow (older firmware left 82..100 unset).
pub const LIVE_MARKER: &[u8; 2] = b"LV";
pub const LIVE_VERSION: u8 = 1;
pub const FLAG_TIMER_RUNNING: u8 = 1 << 0;
pub const FLAG_INSPECTION_RUNNING: u8 = 1 << 1;
/// At 86, outside the 82..86 block the connector checks, so [`LIVE_VERSION`] stays 1.
pub const RSSI_OFFSET: usize = 86;

/// Live values written into the header of a log batch.
pub struct LogsHeader<'a> {
    pub current_time: Option<u64>,
    pub inspection_time: Option<u64>,
    pub competitor: Option<u64>,
    pub group_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub timer_running: bool,
    pub inspection_running: bool,
    /// Signal of the connected AP in dBm; `None` while unknown.
    pub rssi: Option<i8>,
}

impl LogsHeader<'_> {
    pub fn flags(&self) -> u8 {
        let mut flags = 0;
        if self.timer_running {
            flags |= FLAG_TIMER_RUNNING;
        }
        if self.inspection_running {
            flags |= FLAG_INSPECTION_RUNNING;
        }
        flags
    }
}

/// Starts an empty batch: `L` and a clear truncated flag (the rest is written on send).
pub fn init_batch(batch: &mut [u8]) {
    batch[0] = b'L';
    batch[1] = 0x00;
}

/// Fills bytes 2..[`HEADER_LEN`]; leaves the `L` and the truncated flag alone.
pub fn write_header(batch: &mut [u8], header: &LogsHeader) {
    batch[2..10].copy_from_slice(&header.current_time.unwrap_or(u64::MAX).to_be_bytes());
    batch[10..18].copy_from_slice(&header.inspection_time.unwrap_or(u64::MAX).to_be_bytes());
    batch[18..26].copy_from_slice(&header.competitor.unwrap_or(u64::MAX).to_be_bytes());
    batch[26..HEADER_LEN].fill(0x00);

    if let Some(group_id) = header.group_id.filter(|g| g.len() <= 20) {
        batch[26..26 + group_id.len()].copy_from_slice(group_id.as_bytes());
    }

    if let Some(session_id) = header.session_id.filter(|s| s.len() <= 36) {
        batch[46..46 + session_id.len()].copy_from_slice(session_id.as_bytes());
    }

    batch[82..84].copy_from_slice(LIVE_MARKER);
    batch[84] = LIVE_VERSION;
    batch[85] = header.flags();
    // a real reading is always negative; 0 or above can't be told from "unknown"
    batch[RSSI_OFFSET] = header.rssi.filter(|&r| r < 0).map_or(0, |r| r as u8);
}
