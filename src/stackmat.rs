use crate::state::{GlobalState, Scene};
use adv_shift_registers::wrappers::ShifterValueRange;
use embassy_time::{Instant, Timer};
use esp_hal::{gpio::AnyPin, peripherals::UART0, uart::UartRx};

#[embassy_executor::task]
pub async fn stackmat_task(
    uart: UART0,
    uart_pin: AnyPin,
    display: ShifterValueRange,
    global_state: GlobalState,
) {
    let serial_config = esp_hal::uart::config::Config::default().baudrate(1200);
    let mut uart = UartRx::new_async_with_config(uart, serial_config, uart_pin).unwrap();

    let mut buf = [0; 8];
    let mut read_buf = [0; 8];
    let mut last_read = esp_hal::time::now();
    let mut last_state = None;
    let mut last_stackmat_state = StackmatTimerState::Unknown;
    loop {
        if (esp_hal::time::now() - last_read).to_millis() > 500 {
            if last_state != Some(false) {
                global_state.state.lock().await.stackmat_connected = Some(false);
                last_state = Some(false);
                display.set_data(&[255; 6]);
            }
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

                if parsed.0 != last_stackmat_state {
                    if parsed.0 == StackmatTimerState::Running {
                        let mut state = global_state.state.lock().await;
                        if state.scene <= Scene::Inspection && state.solve_time.is_none() {
                            if state.use_inspection {
                                state.inspection_end = Some(Instant::now());
                            }

                            state.scene = Scene::Timer;
                        }
                    } else if parsed.0 == StackmatTimerState::Stopped {
                        let mut state = global_state.state.lock().await;
                        if state.solve_time.is_none() && state.last_solve_time != Some(parsed.1) {
                            let inspection_time = state
                                .inspection_end
                                .and_then(|x| Some(x - state.inspection_start?));
                            let inspection_time = if let Some(ins) = inspection_time {
                                ins.as_millis()
                            } else {
                                0
                            };

                            state.solve_time = Some(parsed.1);
                            state.penalty = if inspection_time >= 17000 {
                                Some(-1)
                            } else if inspection_time >= 15000 {
                                Some(2)
                            } else {
                                None
                            };

                            if state.current_competitor.is_some() {
                                state.scene = Scene::Finished;
                            } else if state.scene >= Scene::WaitingForCompetitor {
                                state.scene = Scene::WaitingForCompetitor;
                            }
                        }
                    } else if parsed.0 == StackmatTimerState::Reset {
                        let mut state = global_state.state.lock().await;
                        if state.current_competitor.is_none()
                            && state.penalty.is_none()
                            && (state.scene == Scene::Timer
                                || state.scene == Scene::WaitingForCompetitor)
                        {
                            state.scene = Scene::WaitingForCompetitor;
                            state.solve_time = None;
                            state.penalty = None;
                        }

                        display.set_data(&[255; 6]);
                    }

                    last_stackmat_state = parsed.0;
                }

                global_state.timer_signal.signal(parsed.1);
                if parsed.1 > 0 {
                    let time_str = crate::utils::ms_to_time_str(parsed.1);
                    display.set_data(&time_str_to_display(&time_str));
                }
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

const DEC_DIGITS: [u8; 10] = [215, 132, 203, 206, 156, 94, 95, 196, 223, 222];
const DOT_MOD: u8 = 32;

fn time_str_to_display(time: &str) -> [u8; 6] {
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
