// timer-func boards time with pads; the stackmat parser is unused there
#![cfg_attr(feature = "timer-func", allow(dead_code))]

/// Largest time a stackmat frame can carry (9:59.999).
#[cfg(any(all(feature = "e2e", not(feature = "timer-func")), test))]
const MAX_FRAME_MS: u64 = 9 * 60_000 + 59_999;

/// Parses one 8-byte stackmat frame: state, `M SS mmm` digits, checksum.
pub fn parse_stackmat_data(data: &[u8; 8]) -> Result<(StackmatTimerState, u64), ()> {
    let mut state = StackmatTimerState::from_u8(data[0]);

    let minutes = parse_digits(&data[1..2]).ok_or(())?;
    let seconds = parse_digits(&data[2..4]).ok_or(())?;
    let ms = parse_digits(&data[4..7]).ok_or(())?;

    if checksum(&data[1..7]) != data[7] {
        return Err(());
    }

    let total_ms = minutes * 60_000 + seconds * 1000 + ms;
    if total_ms > 0 && state == StackmatTimerState::Reset {
        state = StackmatTimerState::Stopped;
    }

    Ok((state, total_ms))
}

/// Decimal digits to a number; `None` on any non-digit.
fn parse_digits(data: &[u8]) -> Option<u64> {
    data.iter().try_fold(0u64, |acc, &x| {
        let digit = x.checked_sub(b'0').filter(|d| *d <= 9)?;
        Some(acc * 10 + digit as u64)
    })
}

/// 64 + sum of the (already validated) time digits; at most 64 + 6 * 9.
fn checksum(digits: &[u8]) -> u8 {
    64 + digits.iter().map(|&x| x.wrapping_sub(b'0')).fold(0u8, u8::wrapping_add)
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum StackmatTimerState {
    Unknown,
    Reset,
    Running,
    Stopped,
}

impl StackmatTimerState {
    pub fn from_u8(val: u8) -> Self {
        match val {
            b'I' => Self::Reset,
            b' ' => Self::Running,
            b'S' => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    #[cfg(any(all(feature = "e2e", not(feature = "timer-func")), test))]
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Reset => b'I',
            Self::Running => b' ',
            Self::Stopped => b'S',
        }
    }
}

/// Builds a stackmat frame (e2e simulation); times above 9:59.999 are clamped.
#[cfg(any(all(feature = "e2e", not(feature = "timer-func")), test))]
pub fn generate_stackmat_data(state: StackmatTimerState, time_ms: u64) -> [u8; 8] {
    let time_ms = time_ms.min(MAX_FRAME_MS);
    let digit = |v: u64| b'0' + (v % 10) as u8;

    let mut data = [0u8; 8];
    data[0] = state.to_u8();
    data[1] = digit(time_ms / 60_000);
    data[2] = digit(time_ms % 60_000 / 10_000);
    data[3] = digit(time_ms % 10_000 / 1000);
    data[4] = digit(time_ms % 1000 / 100);
    data[5] = digit(time_ms % 100 / 10);
    data[6] = digit(time_ms);
    data[7] = checksum(&data[1..7]);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        for (state, ms) in [
            (StackmatTimerState::Running, 0),
            (StackmatTimerState::Running, 1),
            (StackmatTimerState::Stopped, 12_345),
            (StackmatTimerState::Stopped, 59_999),
            (StackmatTimerState::Running, 61_005),
            (StackmatTimerState::Stopped, MAX_FRAME_MS),
        ] {
            assert_eq!(parse_stackmat_data(&generate_stackmat_data(state, ms)), Ok((state, ms)));
        }
    }

    #[test]
    fn generator_clamps_long_times() {
        // used to emit ':' etc. as the minute digit from 10 minutes on
        let frame = generate_stackmat_data(StackmatTimerState::Running, 11 * 60_000);
        assert_eq!(parse_stackmat_data(&frame), Ok((StackmatTimerState::Running, MAX_FRAME_MS)));
    }

    #[test]
    fn reset_with_time_reads_as_stopped() {
        let frame = generate_stackmat_data(StackmatTimerState::Reset, 500);
        assert_eq!(parse_stackmat_data(&frame), Ok((StackmatTimerState::Stopped, 500)));
    }

    #[test]
    fn rejects_bad_frames() {
        let mut frame = generate_stackmat_data(StackmatTimerState::Running, 1234);
        frame[7] ^= 1;
        assert_eq!(parse_stackmat_data(&frame), Err(()));
        assert_eq!(parse_stackmat_data(b"I0:00000"), Err(()));
        assert_eq!(parse_stackmat_data(&[0xFF; 8]), Err(()));
        assert_eq!(parse_stackmat_data(&[0; 8]), Err(()));
    }
}
