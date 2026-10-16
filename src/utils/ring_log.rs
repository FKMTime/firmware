//! Variable-length record log in a fixed buffer (no deps, host-testable with
//! `rustc --edition 2024 --test src/utils/ring_log.rs`).

pub const CODE_LEN: usize = 1 + 8 + 1;
pub const STACKTRACE_HEADER_LEN: usize = 1 + 8 + 16 + 1;

/// Length of the record starting at `buf[0]`, `None` if it is invalid or truncated.
///
/// `b'N'` + u64 + code, or `b'S'` + u64 + [u8; 16] + n + n * u32.
pub fn record_len(buf: &[u8]) -> Option<usize> {
    let len = match *buf.first()? {
        b'N' => CODE_LEN,
        b'S' => STACKTRACE_HEADER_LEN + *buf.get(STACKTRACE_HEADER_LEN - 1)? as usize * 4,
        _ => return None,
    };
    (len <= buf.len()).then_some(len)
}

/// Length of the longest prefix of `buf` made of whole valid records.
pub fn valid_prefix(buf: &[u8]) -> usize {
    let mut valid = 0;
    while let Some(len) = buf.get(valid..).and_then(record_len) {
        valid += len;
    }
    valid
}

/// Appends `record` to `buf[..*len]`, dropping the oldest records until it fits.
/// Returns false (log unchanged) if the record can never fit.
pub fn push(buf: &mut [u8], len: &mut usize, record: &[u8]) -> bool {
    if record.len() > buf.len() {
        return false;
    }

    while *len + record.len() > buf.len() {
        match record_len(&buf[..*len]) {
            Some(oldest) => {
                buf.copy_within(oldest..*len, 0);
                *len -= oldest;
            }
            None => *len = 0,
        }
    }

    buf[*len..*len + record.len()].copy_from_slice(record);
    *len += record.len();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(c: u8) -> [u8; CODE_LEN] {
        let mut r = [0; CODE_LEN];
        r[0] = b'N';
        r[9] = c;
        r
    }

    #[test]
    fn drops_oldest_when_full() {
        let mut buf = [0; 35];
        let mut len = 0;
        for c in 0..10 {
            assert!(push(&mut buf, &mut len, &code(c)));
        }
        assert_eq!(len, 30);
        assert_eq!(buf[9], 7); // oldest kept is code 7
        assert_eq!(buf[29], 9);
        assert_eq!(valid_prefix(&buf[..len]), len);
    }

    #[test]
    fn stacktrace_records_and_oversized() {
        let mut buf = [0; 64];
        let mut len = 0;
        let mut st = [0; STACKTRACE_HEADER_LEN + 8];
        st[0] = b'S';
        st[STACKTRACE_HEADER_LEN - 1] = 2;
        assert!(push(&mut buf, &mut len, &code(1)));
        assert!(push(&mut buf, &mut len, &st));
        assert!(push(&mut buf, &mut len, &st)); // evicts code + first stacktrace
        assert_eq!(len, st.len());
        assert!(!push(&mut buf, &mut len, &[b'N'; 65]));
    }

    #[test]
    fn garbage_never_panics() {
        assert_eq!(valid_prefix(&[]), 0);
        assert_eq!(valid_prefix(&[b'N', 0, 0]), 0);
        assert_eq!(valid_prefix(&[b'S'; 30]), 0);
        assert_eq!(valid_prefix(&[0xFF; 100]), 0);
        let mut ok = code(3).to_vec();
        ok.extend_from_slice(&[b'S', 1]);
        assert_eq!(valid_prefix(&ok), CODE_LEN);

        let mut buf = [0xAA; 20]; // corrupt content with len claiming data
        let mut len = 20;
        assert!(push(&mut buf, &mut len, &code(5)));
        assert_eq!(len, CODE_LEN);
    }
}
