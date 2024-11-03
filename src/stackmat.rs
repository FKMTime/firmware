use crate::scenes::GlobalState;
use embassy_time::Timer;
use esp_hal::{gpio::AnyPin, peripherals::UART0, uart::UartRx};

#[embassy_executor::task]
pub async fn stackmat_task(uart: UART0, uart_pin: AnyPin, global_state: GlobalState) {
    let serial_config = esp_hal::uart::config::Config::default().baudrate(1200);
    let mut uart = UartRx::new_async_with_config(uart, serial_config, uart_pin).unwrap();

    let mut buf = [0; 8];
    let mut read_buf = [0; 8];
    let mut last_read = esp_hal::time::now();
    let mut last_state = None;
    loop {
        if (esp_hal::time::now() - last_read).to_millis() > 1500 {
            if last_state != Some(false) {
                global_state.state.lock().await.stackmat_connected = Some(false);
                last_state = Some(false);
            }

            //log::error!("Stackmat read timeout! (Probably disconnected)");
            //time_sig.signal(None);
            global_state.timer_signal.signal(None);
        }

        Timer::after_millis(10).await;
        let n = UartRx::drain_fifo(&mut uart, &mut read_buf);
        if n == 0 {
            continue;
        }

        for &r in &read_buf[..n] {
            if n == 0 || r == 0 || r == b'\r' {
                continue;
            }

            unsafe {
                core::ptr::copy(buf.as_ptr().offset(1), buf.as_mut_ptr(), 7);
            }

            buf[7] = r;
            if let Ok(parsed) = parse_stackmat_data(&buf) {
                if last_state != Some(true) {
                    global_state.state.lock().await.stackmat_connected = Some(true);
                    last_state = Some(true);
                }

                global_state.timer_signal.signal(Some(parsed.1));
                /*
                crate::scenes::CURRENT_STATE.lock().await.scene = Scene::Timer { inspection_time: parsed.1 };
                crate::scenes::STATE_CHANGED.signal(());
                */

                //log::warn!("parsed: {:?}", parsed);
                //time_sig.signal(Some(parsed.1));
            }
        }

        last_read = esp_hal::time::now();
    }
}

fn parse_stackmat_data(data: &[u8; 8]) -> Result<(StackmatTimerState, u64), ()> {
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
