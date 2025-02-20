use crate::{
    consts::{INSPECTION_TIME_DNF, INSPECTION_TIME_PLUS2},
    state::{sleep_state, GlobalState, Scene},
    utils::stackmat::{
        ms_to_time_str, parse_stackmat_data, time_str_to_display, StackmatTimerState,
    },
};
use adv_shift_registers::wrappers::ShifterValueRange;
use alloc::string::ToString;
use embassy_time::{Instant, Timer};
use esp_hal::{gpio::AnyPin, peripherals::UART1, uart::UartRx};

#[embassy_executor::task]
pub async fn stackmat_task(
    uart: UART1,
    uart_pin: AnyPin,
    display: ShifterValueRange,
    global_state: GlobalState,
) {
    let serial_config = esp_hal::uart::Config::default().with_baudrate(1200);
    let mut uart = UartRx::new(uart, serial_config)
        .expect("Uart Rx new failed")
        .with_rx(uart_pin);

    #[cfg(feature = "e2e")]
    let mut e2e_data = (StackmatTimerState::Reset, 0, esp_hal::time::now());

    let mut buf = [0; 8];
    let mut read_buf = [0; 8];
    let mut last_read = esp_hal::time::now();
    let mut last_state = None;
    let mut last_stackmat_state = StackmatTimerState::Unknown;
    loop {
        if sleep_state() {
            Timer::after_millis(500).await;
            continue;
        }

        if (esp_hal::time::now() - last_read).to_millis() > 500 && last_state != Some(false) {
            global_state.state.lock().await.stackmat_connected = Some(false);
            last_state = Some(false);
            display.set_data(&[255; 6]);
        }

        Timer::after_millis(10).await;

        #[cfg(feature = "e2e")]
        let mut send_ack = false;

        #[cfg(feature = "e2e")]
        let n = {
            if global_state.e2e.stackmat_sig.signaled() {
                let data = global_state.e2e.stackmat_sig.wait().await;
                e2e_data.0 = data.0;
                e2e_data.1 = data.1;
                e2e_data.2 = esp_hal::time::now();
            }

            let timer_ms = match e2e_data.0 {
                StackmatTimerState::Unknown => 0,
                StackmatTimerState::Reset => 0,
                StackmatTimerState::Running => {
                    let mut time = (esp_hal::time::now() - e2e_data.2).to_millis();
                    if time >= e2e_data.1 {
                        time = e2e_data.1;
                        e2e_data.0 = StackmatTimerState::Stopped;
                        send_ack = true;
                    }

                    time
                }
                StackmatTimerState::Stopped => e2e_data.1,
            };

            read_buf.copy_from_slice(&crate::utils::stackmat::generate_stackmat_data(
                &e2e_data.0,
                timer_ms,
            ));

            8
        };

        #[cfg(not(feature = "e2e"))]
        let n = {
            let n = uart.read_buffered_bytes(&mut read_buf);
            let Ok(n) = n else {
                log::error!("uart: read_bytes err");
                continue;
            };

            if n == 0 {
                continue;
            }

            n
        };

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
                            if state.use_inspection() {
                                state.inspection_end = Some(Instant::now());
                            }

                            state.scene = Scene::Timer;
                        }
                    } else if parsed.0 == StackmatTimerState::Stopped {
                        let mut state = global_state.state.lock().await;
                        let last_solve_diff = if cfg!(not(feature = "e2e")) {
                            state.last_solve_time.unwrap_or(0).abs_diff(parsed.1)
                        } else {
                            1000
                        };

                        if state.solve_time.is_none() && last_solve_diff > 10 {
                            let inspection_time = state
                                .inspection_end
                                .zip(state.inspection_start)
                                .map(|(end, start)| (end - start).as_millis())
                                .unwrap_or(0);

                            state.delegate_used = false;
                            state.solve_time = Some(parsed.1);
                            state.penalty = if inspection_time >= INSPECTION_TIME_DNF {
                                Some(-1)
                            } else if inspection_time >= INSPECTION_TIME_PLUS2 {
                                Some(2)
                            } else {
                                None
                            };

                            if state.session_id.is_none() {
                                state.session_id = Some(uuid::Uuid::new_v4().to_string());
                            }

                            if state.current_competitor.is_some() {
                                state.scene = Scene::Finished;
                            } else if state.scene >= Scene::WaitingForCompetitor {
                                state.scene = Scene::WaitingForCompetitor;
                            }

                            if let Some(saved_state) = state.to_saved_global_state() {
                                saved_state.to_nvs(&global_state.nvs).await;
                            }

                            let time_str = ms_to_time_str(parsed.1);
                            display.set_data(&time_str_to_display(&time_str));
                        } else if state.scene == Scene::Timer {
                            // this will only happen on e2e (i dont think any human is capable of
                            // pausing timer two times in a row at the same time)
                            state.scene = Scene::WaitingForCompetitor;
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
                            state.inspection_start = None;
                            state.inspection_end = None;
                        } else if state.current_competitor.is_some() && state.scene == Scene::Timer
                        {
                            state.scene = Scene::Finished;
                            state.solve_time = Some(0);
                            state.penalty = Some(-1);

                            if state.session_id.is_none() {
                                state.session_id = Some(uuid::Uuid::new_v4().to_string());
                            }

                            state.time_confirmed = true;
                        }

                        display.set_data(&[255; 6]);
                    }

                    last_stackmat_state = parsed.0;
                }

                #[cfg(feature = "e2e")]
                if send_ack {
                    crate::ws::send_test_ack().await;
                }

                global_state.timer_signal.signal(parsed.1);
            }
        }

        last_read = esp_hal::time::now();
    }
}
