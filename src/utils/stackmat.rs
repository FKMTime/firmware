pub const DEC_DIGITS: [u8; 10] = [215, 132, 203, 206, 156, 94, 95, 196, 223, 222];
pub const DOT_MOD: u8 = 32;

pub fn parse_stackmat_data(data: &[u8; 8]) -> Result<(StackmatTimerState, u64), ()> {
    let mut state = StackmatTimerState::from_u8(data[0]);

    let minutes = parse_time_str(&data[1..2]).ok_or(())?;
    let seconds = parse_time_str(&data[2..4]).ok_or(())?;
    let ms = parse_time_str(&data[4..7]).ok_or(())?;

    let sum = 64 + data[1..7].iter().fold(0u8, |acc, &x| acc + (x - b'0'));
    if sum != data[7] {
        // cheksum
        return Err(());
    }

    let total_ms: u64 = minutes as u64 * 60000 + seconds as u64 * 1000 + ms as u64;
    if total_ms > 0 && state == StackmatTimerState::Reset {
        state = StackmatTimerState::Stopped;
    }

    Ok((state, total_ms))
}

fn parse_time_str(data: &[u8]) -> Option<u16> {
    data.iter().try_fold(0u16, |acc, &x| {
        let digit = x.checked_sub(b'0')?;
        if digit > 9 {
            return None;
        }

        acc.checked_mul(10)
            .and_then(|acc| acc.checked_add(digit as u16))
    })
}

#[allow(dead_code)]
#[derive(PartialEq, Debug)]
pub enum StackmatTimerState {
    Unknown,
    Reset,
    Running,
    Stopped,
}

#[allow(dead_code)]
impl StackmatTimerState {
    pub fn from_u8(val: u8) -> Self {
        match val {
            b'I' => Self::Reset,
            b' ' => Self::Running,
            b'S' => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Reset => b'I',
            Self::Running => b' ',
            Self::Stopped => b'S',
        }
    }
}

pub fn time_str_to_display(time: &str) -> [u8; 6] {
    let mut data = [255; 6];
    let mut i = 0;

    for c in time.chars().rev() {
        if c < '0' || c > '9' {
            continue;
        }

        let dot = if i == 5 || i == 3 { DOT_MOD } else { 0 };

        let d = c as u8 - b'0';
        data[i] = !DEC_DIGITS[d as usize] ^ dot;
        i += 1;
    }

    data
}

pub fn ms_to_time_str(ms: u64) -> heapless::String<8> {
    let minutes: u8 = (ms / 60000) as u8;
    let seconds: u8 = ((ms % 60000) / 1000) as u8;
    let ms: u16 = (ms % 1000) as u16;

    let mut time_str = heapless::String::<8>::new();
    if minutes > 0 {
        _ = time_str.push((minutes + b'0') as char);
        _ = time_str.push(':');
        _ = time_str.push_str(&alloc::format!("{seconds:02}.{ms:03}"));
    } else {
        _ = time_str.push_str(&alloc::format!("{seconds:01}.{ms:03}"));
    }

    time_str
}
