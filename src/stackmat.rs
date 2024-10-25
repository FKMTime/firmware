use core::str::from_utf8;
use esp_hal::{gpio::AnyPin, peripherals::UART0, uart::UartRx};

#[embassy_executor::task]
pub async fn stackmat_task(uart: UART0, uart_pin: AnyPin) {
    let serial_config = esp_hal::uart::config::Config::default().baudrate(1200);
    let mut uart = UartRx::new_async_with_config(uart, serial_config, uart_pin).unwrap();

    let mut buf = [0; 8];
    let mut buf_i = 0;

    let mut read_buf = [0; 1];
    loop {
        if let Ok(n) = embedded_io_async::Read::read(&mut uart, &mut read_buf).await {
            if n == 0 {
                continue;
            }

            if read_buf[0] == 0 || read_buf[0] == b'\r' || buf_i == 8 {
                let parsed = parse_stackmat_data(&buf);
                log::warn!("parsed: {:?}", parsed);
                buf_i = 0;
            } else {
                buf[buf_i] = read_buf[0];
                buf_i += 1;
            }
        }
    }
}

fn parse_stackmat_data(data: &[u8; 8]) -> Result<(StackmatTimerState, u64), ()> {
    let mut state = StackmatTimerState::from_u8(data[0]);

    let minutes: u8 =
        u8::from_str_radix(from_utf8(&data[1..2]).map_err(|_| ())?, 10).map_err(|_| ())?;

    let seconds: u8 =
        u8::from_str_radix(from_utf8(&data[2..4]).map_err(|_| ())?, 10).map_err(|_| ())?;

    let ms: u16 =
        u16::from_str_radix(from_utf8(&data[4..7]).map_err(|_| ())?, 10).map_err(|_| ())?;

    let sum = 64 + data[1..7].iter().map(|&x| x - '0' as u8).sum::<u8>();
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

#[allow(dead_code)]
#[derive(PartialEq, Debug)]
enum StackmatTimerState {
    Unknown,
    Reset,
    Running,
    Stopped,
}

#[allow(dead_code)]
impl StackmatTimerState {
    fn from_u8(val: u8) -> Self {
        match val {
            b'I' => Self::Reset,
            b' ' => Self::Running,
            b'S' => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    fn to_u8(&self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Reset => b'I',
            Self::Running => b' ',
            Self::Stopped => b'S',
        }
    }
}
