#[cfg(all(feature = "v3", feature = "timer-func"))]
compile_error!("feature `timer-func` is not supported in v3");

use crate::watchdog::{TaskId, heartbeat};
use crate::{
    consts::{INSPECTION_TIME_DNF, INSPECTION_TIME_PLUS2},
    state::{GlobalState, Scene, Solve},
};
use embassy_time::{Instant, Timer};

/// Publishes the running time to the display, the BLE display and the logger.
fn show_time(global_state: &GlobalState, ms: u64) {
    global_state.timer.sender().send(ms);
}

/// Inspection → timer when a solve starts (no-op if this attempt already has a time).
fn start_timer(s: &mut Solve) -> bool {
    if s.scene > Scene::Inspection || s.solve_time.is_some() {
        return false;
    }
    if s.use_inspection() {
        s.inspection_end = Some(Instant::now());
    }
    s.scene = Scene::Timer;
    true
}

#[cfg(feature = "timer-func")]
#[embassy_executor::task]
pub async fn stackmat_task(
    global_state: GlobalState,
    mut pads: (esp_hal::gpio::Input<'static>, esp_hal::gpio::Input<'static>),
) {
    global_state.status.update(|s| s.stackmat_connected = Some(true));

    loop {
        heartbeat(TaskId::Stackmat);
        if pads.0.is_high() && pads.1.is_high() {
            wait_pad_release(&mut pads).await;
            let timer_start = Instant::now();
            global_state.timer_stop_signal.reset();

            if global_state.solve.update(start_timer) {
                loop {
                    heartbeat(TaskId::Stackmat);
                    let time = timer_start.elapsed().as_millis();

                    if global_state.timer_stop_signal.try_take().is_some() {
                        break;
                    }

                    if let Some(limit) = global_state.solve.read(Solve::time_limit)
                        && time > limit
                    {
                        show_time(&global_state, limit);
                        time_end(limit, true, &mut None, &global_state).await;
                        break;
                    } else {
                        show_time(&global_state, time);

                        if pads.0.is_high() && pads.1.is_high() {
                            time_end(time, false, &mut None, &global_state).await;
                            wait_pad_release(&mut pads).await;

                            break;
                        }
                    }

                    Timer::after_millis(1000 / 30).await;
                }
            }
        }

        Timer::after_millis(10).await;
    }
}

/// Waits until a hand leaves the pads, beating the watchdog meanwhile.
#[cfg(feature = "timer-func")]
async fn wait_pad_release(
    pads: &mut (esp_hal::gpio::Input<'static>, esp_hal::gpio::Input<'static>),
) {
    loop {
        heartbeat(TaskId::Stackmat);
        let released =
            embassy_futures::select::select(pads.0.wait_for_low(), pads.1.wait_for_low());
        if embassy_time::with_timeout(embassy_time::Duration::from_secs(10), released)
            .await
            .is_ok()
        {
            return;
        }
    }
}

#[cfg(not(feature = "timer-func"))]
#[embassy_executor::task]
pub async fn stackmat_task(
    uart: esp_hal::peripherals::UART1<'static>,
    uart_pin: esp_hal::gpio::AnyPin<'static>,
    global_state: GlobalState,
) {
    use crate::utils::stackmat::{StackmatTimerState, parse_stackmat_data};
    use esp_hal::uart::UartRx;

    let serial_config = esp_hal::uart::Config::default().with_baudrate(1200);
    let Ok(mut uart) = UartRx::new(uart, serial_config).map(|u| u.with_rx(uart_pin)) else {
        log::error!("Stackmat task error while creating UartRx instance!");
        crate::utils::error_log::add_error(
            crate::utils::error_log::codes::STACKMAT_UART_INIT_FAILED,
        );
        return;
    };

    #[cfg(feature = "e2e")]
    let mut e2e_data = (StackmatTimerState::Reset, 0, esp_hal::time::Instant::now());

    let mut buf = [0; 8];
    let mut read_buf = [0; 8];
    let mut last_read = esp_hal::time::Instant::now();
    let mut last_time: Option<(Instant, u64)> = None;
    let mut last_state = None;
    let mut last_stackmat_state = StackmatTimerState::Unknown;
    loop {
        heartbeat(TaskId::Stackmat);
        if crate::power::is_sleeping() {
            loop {
                let n = uart.read_buffered(&mut read_buf);
                if n.is_err() || n == Ok(0) {
                    break;
                }
            }

            Timer::after_millis(500).await;
            continue;
        }

        if last_read.elapsed().as_millis() > 500 && last_state != Some(false) {
            last_state = Some(false);
            global_state.timer_stop_signal.reset();
            if last_time.is_none() {
                global_state.status.update(|s| s.stackmat_connected = Some(false));
                global_state.solve.update(|s| {
                    if s.scene == Scene::Timer {
                        s.scene = match s.current_competitor {
                            Some(_) => Scene::CompetitorInfo,
                            None => Scene::WaitingForCompetitor,
                        };
                    }
                });
            }
        }

        if last_state == Some(false)
            && let Some((last_at, last_ms)) = last_time
        {
            let time_interpolated = last_ms.saturating_add(last_at.elapsed().as_millis());
            if let Some(limit) = global_state.solve.read(Solve::time_limit)
                && time_interpolated > limit
            {
                show_time(&global_state, limit);
                time_end(limit, true, &mut last_time, &global_state).await;
            } else {
                show_time(&global_state, time_interpolated);

                if global_state.timer_stop_signal.try_take().is_some() {
                    time_end(time_interpolated, false, &mut last_time, &global_state).await;
                }
            }
        }

        Timer::after_millis(10).await;

        #[cfg(feature = "e2e")]
        let mut send_ack = false;

        #[cfg(feature = "e2e")]
        let n = {
            if let Some(data) = global_state.e2e.stackmat_sig.try_take() {
                e2e_data.0 = data.0;
                e2e_data.1 = data.1;
                e2e_data.2 = esp_hal::time::Instant::now();
            }

            let timer_ms = match e2e_data.0 {
                StackmatTimerState::Unknown => 0,
                StackmatTimerState::Reset => 0,
                StackmatTimerState::Running => {
                    let mut time = e2e_data.2.elapsed().as_millis();
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
                e2e_data.0, timer_ms,
            ));

            8
        };

        #[cfg(not(feature = "e2e"))]
        let n = {
            let n = uart.read_buffered(&mut read_buf);
            let n = match n {
                Ok(n) => n,
                Err(e) => {
                    #[cfg(not(feature = "release_build"))]
                    {
                        log::error!("uart: read_bytes err {e:?}");
                    }

                    continue;
                }
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

            buf.copy_within(1.., 0);
            buf[7] = r;
            if let Ok(parsed) = parse_stackmat_data(&buf) {
                if last_state != Some(true) {
                    global_state.status.update(|s| s.stackmat_connected = Some(true));
                    last_state = Some(true);

                    #[cfg(feature = "qa")]
                    crate::qa::send_qa_resp(crate::qa::QaSignal::StackmatConnected);
                }

                if let Some(limit) = global_state.solve.read(Solve::time_limit)
                    && parsed.1 > limit
                {
                    show_time(&global_state, parsed.1);
                    time_end(limit, true, &mut last_time, &global_state).await;
                    global_state.solve.write(|s| s.limit_armed = false);
                    continue;
                }
                last_time = Some((Instant::now(), parsed.1));

                if parsed.0 != last_stackmat_state && parsed.0 != StackmatTimerState::Unknown {
                    if parsed.0 == StackmatTimerState::Running {
                        if global_state.solve.update(start_timer) {
                            global_state.timer_stop_signal.reset();
                        }
                    } else if parsed.0 == StackmatTimerState::Stopped {
                        time_end(parsed.1, false, &mut last_time, &global_state).await;
                    } else if parsed.0 == StackmatTimerState::Reset {
                        let reset = global_state.solve.update(|s| {
                            if s.current_competitor.is_none()
                                && s.penalty.is_none()
                                && (s.scene == Scene::Timer || s.scene == Scene::WaitingForCompetitor)
                            {
                                s.scene = Scene::WaitingForCompetitor;
                                s.solve_time = None;
                                s.penalty = None;
                                s.inspection_start = None;
                                s.inspection_end = None;
                                true
                            } else if s.current_competitor.is_some() && s.scene == Scene::Timer {
                                // reset while timing = DNF
                                s.scene = Scene::Finished;
                                s.solve_time = Some(0);
                                s.penalty = Some(-1);
                                s.session_id.get_or_insert_with(crate::state::new_session_id);
                                s.time_confirmed = true;
                                true
                            } else {
                                false
                            }
                        });
                        if reset {
                            last_time = None;
                        }
                    }

                    last_stackmat_state = parsed.0;
                }

                #[cfg(feature = "e2e")]
                if send_ack {
                    crate::ws::send_test_ack(&global_state).await;
                }

                show_time(&global_state, parsed.1);
            }
        }

        last_read = esp_hal::time::Instant::now();
    }
}

async fn time_end(
    time: u64,
    dnf: bool,
    last_time: &mut Option<(Instant, u64)>,
    global_state: &GlobalState,
) {
    *last_time = None;

    // `None` = this attempt already had a time; `Some(saved copy)` = new time recorded
    let recorded = global_state.solve.update(|s| {
        let inspection_time = s.inspection_end.and(s.inspection_ms()).unwrap_or(0);
        log::info!("Timer stopped: {time}ms (inspection: {inspection_time}ms)");

        if s.solve_time.is_some() {
            if s.scene == Scene::Timer {
                s.scene = Scene::WaitingForCompetitor;
            }
            return None;
        }

        s.delegate_used = false;
        s.solve_time = Some(time);
        s.penalty = if inspection_time >= INSPECTION_TIME_DNF || dnf {
            Some(-1)
        } else if inspection_time >= INSPECTION_TIME_PLUS2 {
            Some(2)
        } else {
            None
        };
        s.session_id.get_or_insert_with(crate::state::new_session_id);

        if s.current_competitor.is_some() {
            s.scene = if s.possible_groups.len() > 1 && s.solve_group.is_none() {
                Scene::GroupSelect
            } else {
                Scene::Finished
            };
        } else if s.scene >= Scene::WaitingForCompetitor {
            s.scene = Scene::WaitingForCompetitor;
        }

        Some(s.to_saved())
    });

    #[cfg(feature = "qa")]
    if recorded.is_some() {
        crate::qa::send_qa_resp(crate::qa::QaSignal::Stackmat(time));
    }

    // the flash write happens after the state is released
    #[cfg(not(feature = "qa"))]
    if let Some(Some(saved)) = recorded {
        saved.to_nvs(global_state.nvs).await;
    }
}
