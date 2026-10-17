//! Pure helpers of the BLE display link (no hardware types, host-tested).

/// Bond record stored in NVS: `[0..6]` display address, `[6..22]` LTK (little endian),
/// `[22]` security level (0 none, 1 encrypted, 2 authenticated).
pub const BOND_LEN: usize = 23;

pub fn encode_bond(addr: [u8; 6], ltk: [u8; 16], level: u8) -> [u8; BOND_LEN] {
    let mut buf = [0; BOND_LEN];
    buf[..6].copy_from_slice(&addr);
    buf[6..22].copy_from_slice(&ltk);
    buf[22] = level;
    buf
}

/// `None` for anything that isn't a current record (old formats just mean "pair again").
pub fn decode_bond(buf: &[u8]) -> Option<([u8; 6], [u8; 16], u8)> {
    let buf: &[u8; BOND_LEN] = buf.try_into().ok()?;
    let addr = buf[..6].try_into().ok()?;
    let ltk = buf[6..22].try_into().ok()?;
    (buf[22] <= 2).then_some((addr, ltk, buf[22]))
}

/// Local name (complete or shortened) from BLE advertising data.
pub fn parse_device_name(mut data: &[u8]) -> Option<&str> {
    while let [len, rest @ ..] = data {
        let len = *len as usize;
        if len == 0 || len > rest.len() {
            return None;
        }

        let (field, next) = rest.split_at(len);
        if let [0x08 | 0x09, name @ ..] = field {
            return core::str::from_utf8(name).ok();
        }
        data = next;
    }
    None
}

pub fn is_fkm_display(name: &str) -> bool {
    name.starts_with("FKMD-")
}
