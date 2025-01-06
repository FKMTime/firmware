use crate::{
    state::{GlobalState, Scene},
    utils::stackmat::{
        ms_to_time_str, parse_stackmat_data, time_str_to_display, StackmatTimerState,
    },
};
use adv_shift_registers::wrappers::ShifterValueRange;
use embassy_time::{Instant, Timer};
use esp_hal::{gpio::AnyPin, peripherals::UART1, uart::UartRx};

#[embassy_executor::task]
pub async fn stackmat_task(
    uart: UART1,
    uart_pin: AnyPin,
    display: ShifterValueRange,
    global_state: GlobalState,
) {
    let serial_config = esp_hal::uart::Config::default().baudrate(1200);
    let mut uart = UartRx::new_with_config(uart, serial_config, uart_pin).unwrap();

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
                    let time_str = ms_to_time_str(parsed.1);
                    display.set_data(&time_str_to_display(&time_str));
                }
            }
        }

        last_read = esp_hal::time::now();
    }
}
