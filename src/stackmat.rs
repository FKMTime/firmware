#[cfg(all(feature = "v3", feature = "timer-func"))]
compile_error!("feature `timer-func` is not supported in v3");

use crate::{
    consts::{INSPECTION_TIME_DNF, INSPECTION_TIME_PLUS2},
    state::{GlobalState, Scene},
};
use alloc::string::ToString;
use embassy_time::{Instant, Timer};

pub static mut CURRENT_TIME: u64 = 0;

#[cfg(feature = "timer-func")]
#[embassy_executor::task]
pub async fn stackmat_task(
    global_state: GlobalState,
    mut pads: (esp_hal::gpio::Input<'static>, esp_hal::gpio::Input<'static>),
) {
    {
        let mut state = global_state.state.lock().await;
        state.stackmat_connected = Some(true);
    }

    loop {
        if pads.0.is_high() && pads.1.is_high() {
            embassy_futures::select::select(pads.0.wait_for_low(), pads.1.wait_for_low()).await;
            let timer_start = Instant::now();
            global_state.timer_stop_signal.reset();

            let mut state = global_state.state.lock().await;
            if state.scene <= Scene::Inspection && state.solve_time.is_none() {
                if state.use_inspection() {
                    state.inspection_end = Some(Instant::now());
                }

                state.scene = Scene::Timer;
                drop(state);

                loop {
                    let time = timer_start.elapsed().as_millis();

                    if global_state.timer_stop_signal.signaled() {
                        global_state.timer_stop_signal.wait().await;
                        break;
                    }

                    let time_limit = unsafe { crate::state::GROUP_LIMIT };
                    if let Some(limit) = time_limit
                        && time > limit
                    {
                        global_state.timer_signal.signal(limit);
                        global_state.bt_display_signal.signal(limit);
                        unsafe {
                            CURRENT_TIME = limit;
                        }
                        time_end(limit, true, &mut None, &global_state).await;
                        break;
                    } else {
                        global_state.timer_signal.signal(time);
                        global_state.bt_display_signal.signal(time);
                        unsafe {
                            CURRENT_TIME = time;
                        }

                        if pads.0.is_high() && pads.1.is_high() {
                            time_end(time, false, &mut None, &global_state).await;
                            embassy_futures::select::select(
                                pads.0.wait_for_low(),
                                pads.1.wait_for_low(),
                            )
                            .await;

                            break;
                        }
                    }

                    Timer::after_millis(1000 / 30).await;
                }
            } else {
                drop(state);
            }
        }

        Timer::after_millis(10).await;
    }
}

#[cfg(not(feature = "timer-func"))]
#[embassy_executor::task]
pub async fn stackmat_task(
    uart: esp_hal::peripherals::UART1<'static>,
    uart_pin: esp_hal::gpio::AnyPin<'static>,
    global_state: GlobalState,
) {
    use crate::{
        state::sleep_state,
        utils::stackmat::{StackmatTimerState, parse_stackmat_data},
    };
    use esp_hal::uart::UartRx;

    let serial_config = esp_hal::uart::Config::default().with_baudrate(1200);
    let Ok(mut uart) = UartRx::new(uart, serial_config).map(|u| u.with_rx(uart_pin)) else {
        log::error!("Stackmat task error while creating UartRx instance!");
        crate::utils::error_log::add_error(
            crate::utils::error_log::codes::STACKMAT_UART_INIT_FAILED,
        )
        .await;
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
        if sleep_state() {
            loop {
                let n = uart.read_buffered(&mut read_buf);
                if n.is_err() || n == Ok(0) {
                    break;
                }
            }

            Timer::after_millis(500).await;
            continue;
        }

        if (esp_hal::time::Instant::now() - last_read).as_millis() > 500
            && last_state != Some(false)
        {
            last_state = Some(false);
            global_state.timer_stop_signal.reset();
            if last_time.is_none() {
                let mut state = global_state.state.lock().await;
                state.stackmat_connected = Some(false);

                if state.scene == Scene::Timer {
                    if state.current_competitor.is_some() {
                        state.scene = Scene::CompetitorInfo;
                    } else {
                        state.scene = Scene::WaitingForCompetitor;
                    }
                }
            }
        }

        if last_state == Some(false)
            && let Some((last_at, last_ms)) = last_time
        {
            let time_interpolated = last_ms + last_at.elapsed().as_millis();
            let time_limit = unsafe { crate::state::GROUP_LIMIT };
            if let Some(limit) = time_limit
                && time_interpolated > limit
            {
                global_state.timer_signal.signal(limit);
                global_state.bt_display_signal.signal(limit);
                unsafe {
                    CURRENT_TIME = limit;
                }
                time_end(limit, true, &mut last_time, &global_state).await;
            } else {
                global_state.timer_signal.signal(time_interpolated);
                global_state.bt_display_signal.signal(time_interpolated);
                unsafe {
                    CURRENT_TIME = time_interpolated;
                }

                if global_state.timer_stop_signal.signaled() {
                    time_end(time_interpolated, false, &mut last_time, &global_state).await;
                }
            }
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
                e2e_data.2 = esp_hal::time::Instant::now();
            }

            let timer_ms = match e2e_data.0 {
                StackmatTimerState::Unknown => 0,
                StackmatTimerState::Reset => 0,
                StackmatTimerState::Running => {
                    let mut time = (esp_hal::time::Instant::now() - e2e_data.2).as_millis();
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

            unsafe {
                core::ptr::copy(buf.as_ptr().offset(1), buf.as_mut_ptr(), 7);
            }

            buf[7] = r;
            if let Ok(parsed) = parse_stackmat_data(&buf) {
                if last_state != Some(true) {
                    global_state.state.lock().await.stackmat_connected = Some(true);
                    last_state = Some(true);

                    #[cfg(feature = "qa")]
                    crate::qa::send_qa_resp(crate::qa::QaSignal::StackmatConnected);
                }

                unsafe {
                    CURRENT_TIME = parsed.1;
                }

                let time_limit = unsafe { crate::state::GROUP_LIMIT };
                if let Some(limit) = time_limit
                    && parsed.1 > limit
                {
                    time_end(limit, true, &mut last_time, &global_state).await;
                    unsafe {
                        crate::state::GROUP_LIMIT = None;
                    }
                    continue;
                }
                last_time = Some((Instant::now(), parsed.1));

                if parsed.0 != last_stackmat_state && parsed.0 != StackmatTimerState::Unknown {
                    if parsed.0 == StackmatTimerState::Running {
                        let mut state = global_state.state.lock().await;
                        if state.scene <= Scene::Inspection && state.solve_time.is_none() {
                            if state.use_inspection() {
                                state.inspection_end = Some(Instant::now());
                            }

                            state.scene = Scene::Timer;
                            global_state.timer_stop_signal.reset();
                        }
                    } else if parsed.0 == StackmatTimerState::Stopped {
                        time_end(parsed.1, false, &mut last_time, &global_state).await;
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
                            last_time = None;
                        } else if state.current_competitor.is_some() && state.scene == Scene::Timer
                        {
                            state.scene = Scene::Finished;
                            state.solve_time = Some(0);
                            state.penalty = Some(-1);

                            if state.session_id.is_none() {
                                state.session_id = Some(uuid::Uuid::new_v4().to_string());
                            }

                            state.time_confirmed = true;
                            last_time = None;
                        }
                    }

                    last_stackmat_state = parsed.0;
                }

                #[cfg(feature = "e2e")]
                if send_ack {
                    crate::ws::send_test_ack(&global_state).await;
                }

                global_state.timer_signal.signal(parsed.1);
                global_state.bt_display_signal.signal(parsed.1);
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
    let mut state = global_state.state.lock().await;
    let inspection_time = state
        .inspection_end
        .zip(state.inspection_start)
        .map(|(end, start)| (end - start).as_millis())
        .unwrap_or(0);

    log::info!(
        "Timer stopped: {}ms (inspection: {inspection_time}ms)",
        time
    );
    if state.solve_time.is_none() {
        state.delegate_used = false;
        state.solve_time = Some(time);
        state.penalty = if inspection_time >= INSPECTION_TIME_DNF || dnf {
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
            if state.possible_groups.len() > 1 && state.solve_group.is_none() {
                state.scene = Scene::GroupSelect;
            } else {
                state.scene = Scene::Finished;
            }
        } else if state.scene >= Scene::WaitingForCompetitor {
            state.scene = Scene::WaitingForCompetitor;
        }

        #[cfg(not(feature = "qa"))]
        if let Some(saved_state) = state.to_saved_global_state() {
            saved_state.to_nvs(&global_state.nvs).await;
        }

        #[cfg(feature = "qa")]
        crate::qa::send_qa_resp(crate::qa::QaSignal::Stackmat(time));
    } else if state.scene == Scene::Timer {
        state.scene = Scene::WaitingForCompetitor;
    }

    *last_time = None;
}
