//! Pure helpers of the BLE display link (no hardware types, host-tested).

/// Size of the bond record stored in NVS.
pub const BOND_LEN: usize = 32;

/// Layout: `[0..6]` peer address, `[6..22]` LTK (little endian), `[22]` security level,
/// `[23]` address kind (1 = random), `[31]` record version.
/// Version 0 records (written before the kind byte existed) have zeros in `[23..32]` and are
/// random addresses.
const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BondRecord {
    pub addr: [u8; 6],
    pub random_addr: bool,
    pub ltk: [u8; 16],
    /// 0 = no encryption, 1 = encrypted, 2 = encrypted + authenticated
    pub security_level: u8,
}

pub fn encode_bond(bond: &BondRecord) -> [u8; BOND_LEN] {
    let mut buf = [0; BOND_LEN];
    buf[..6].copy_from_slice(&bond.addr);
    buf[6..22].copy_from_slice(&bond.ltk);
    buf[22] = bond.security_level;
    buf[23] = bond.random_addr as u8;
    buf[31] = VERSION;
    buf
}

pub fn decode_bond(buf: &[u8]) -> Option<BondRecord> {
    let buf: &[u8; BOND_LEN] = buf.try_into().ok()?;
    let random_addr = match buf[31] {
        0 => true,
        VERSION => buf[23] != 0,
        _ => return None,
    };
    if buf[22] > 2 {
        return None;
    }

    let mut addr = [0; 6];
    addr.copy_from_slice(&buf[..6]);
    let mut ltk = [0; 16];
    ltk.copy_from_slice(&buf[6..22]);
    Some(BondRecord {
        addr,
        random_addr,
        ltk,
        security_level: buf[22],
    })
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
