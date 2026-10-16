#[cfg(all(feature = "v3", feature = "timer-func"))]
compile_error!("feature `timer-func` is not supported in v3");

use crate::watchdog::{TaskId, heartbeat};
use crate::{
    consts::{INSPECTION_TIME_DNF, INSPECTION_TIME_PLUS2},
    state::{GlobalState, Scene, Solve},
};
// the e2e uart simulator replaces the timed read
#[cfg_attr(all(feature = "e2e", not(feature = "timer-func")), allow(unused_imports))]
use embassy_time::{Duration, Instant, Timer, with_timeout};

/// Publishes the running time to the display, the BLE display and the logger.
fn show_time(global_state: &GlobalState, ms: u64) {
    global_state.timer.sender().send(ms);
}

/// Inspection → timer for a solve that started at `started` (no-op if it already has a time).
fn start_timer(s: &mut Solve, started: Instant) -> bool {
    if s.scene > Scene::Inspection || s.solve_time.is_some() {
        return false;
    }
    if s.use_inspection() {
        s.inspection_end = Some(started);
    }
    s.scene = Scene::Timer;
    true
}

#[cfg(feature = "timer-func")]
type Pads = (esp_hal::gpio::Input<'static>, esp_hal::gpio::Input<'static>);

/// Pads timer: starts when a hand leaves the held pads, stops when both are touched again.
/// Edge-triggered, so start/stop instants are exact to the interrupt latency.
#[cfg(feature = "timer-func")]
#[embassy_executor::task]
pub async fn stackmat_task(global_state: GlobalState, mut pads: Pads) {
    use embassy_futures::select::{Either, select};

    global_state.status.update(|s| s.stackmat_connected = Some(true));

    loop {
        heartbeat(TaskId::Stackmat);
        if with_timeout(Duration::from_secs(10), both_pressed(&mut pads)).await.is_err() {
            continue;
        }
        wait_pad_release(&mut pads).await;
        let timer_start = Instant::now();
        global_state.timer_stop_signal.reset();
        if !global_state.solve.update(|s| start_timer(s, timer_start)) {
            continue;
        }
        crate::bench!(PADS_EVENT, timer_start.elapsed().as_micros());

        loop {
            heartbeat(TaskId::Stackmat);
            let tick = select(Timer::after_millis(1000 / 30), both_pressed(&mut pads)).await;
            let now = Instant::now();
            let time = now.saturating_duration_since(timer_start).as_millis();
            let limit = global_state.solve.read(Solve::time_limit);

            if let Either::Second(()) = tick {
                let (time, dnf) = match limit {
                    Some(limit) if time > limit => (limit, true),
                    _ => (time, false),
                };
                time_end(time, dnf, &global_state).await;
                crate::bench!(PADS_EVENT, now.elapsed().as_micros());
                show_time(&global_state, time);
                wait_pad_release(&mut pads).await;
                break;
            }
            if global_state.timer_stop_signal.try_take().is_some() {
                break;
            }
            if let Some(limit) = limit
                && time > limit
            {
                show_time(&global_state, limit);
                time_end(limit, true, &global_state).await;
                break;
            }
            show_time(&global_state, time);
        }
    }
}

/// Both pads touched (returns right away if they already are).
#[cfg(feature = "timer-func")]
async fn both_pressed(pads: &mut Pads) {
    loop {
        pads.0.wait_for_high().await;
        pads.1.wait_for_high().await;
        if pads.0.is_high() {
            return;
        }
    }
}

/// Waits until a hand leaves the pads, beating the watchdog meanwhile.
#[cfg(feature = "timer-func")]
async fn wait_pad_release(pads: &mut Pads) {
    use embassy_futures::select::select;
    loop {
        heartbeat(TaskId::Stackmat);
        let released = select(pads.0.wait_for_low(), pads.1.wait_for_low());
        if with_timeout(Duration::from_secs(10), released).await.is_ok() {
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
    use crate::utils::stackmat::{BYTE_US, Event, FRAME_RX_DELAY_US, Parser, Tracker};
    use esp_hal::uart::{Config, RxConfig, UartRx};

    // threshold 1: every byte wakes the task, so it gets an exact timestamp
    let config = Config::default()
        .with_baudrate(1200)
        .with_rx(RxConfig::default().with_fifo_full_threshold(1));
    let Ok(uart) = UartRx::new(uart, config) else {
        log::error!("Stackmat task error while creating UartRx instance!");
        crate::utils::error_log::add_error(
            crate::utils::error_log::codes::STACKMAT_UART_INIT_FAILED,
        );
        return;
    };
    #[cfg_attr(feature = "e2e", allow(unused_mut, unused_variables))]
    let mut uart = uart.with_rx(uart_pin).into_async();

    #[cfg(feature = "e2e")]
    let mut sim = e2e::Sim::default();
    let mut parser = Parser::default();
    let rx_delay = if cfg!(feature = "e2e") { 0 } else { FRAME_RX_DELAY_US };
    let mut tracker = Tracker::new(rx_delay);
    let mut connected: Option<bool> = None;
    let mut buf = [0u8; 16];

    loop {
        heartbeat(TaskId::Stackmat);
        let sleeping = crate::power::is_sleeping();
        let now = Instant::now().as_micros();
        let extrapolating = connected == Some(false) && tracker.running_ms(now).is_some();
        let wait = match (sleeping, extrapolating) {
            (true, _) => 500,
            (_, true) => 1000 / 30,
            _ => 100,
        };

        #[cfg(not(feature = "e2e"))]
        let n = match with_timeout(Duration::from_millis(wait), uart.read_async(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                log::debug!("stackmat uart: {e:?}");
                Timer::after_millis(10).await;
                0
            }
            Err(_) => 0,
        };
        #[cfg(feature = "e2e")]
        let n = {
            let _ = wait;
            Timer::after_millis(10).await;
            sim.frame(&global_state, &mut buf)
        };

        if sleeping {
            continue;
        }
        crate::bench!(STACKMAT_LOOP);

        let now = Instant::now().as_micros();
        for (i, &byte) in buf[..n].iter().enumerate() {
            let Some(frame) = parser.push(byte) else {
                continue;
            };
            // bytes read together arrived back to back
            let at = now.saturating_sub((n - 1 - i) as u64 * BYTE_US);
            if connected != Some(true) {
                connected = Some(true);
                global_state.status.update(|s| s.stackmat_connected = Some(true));
                #[cfg(feature = "qa")]
                crate::qa::send_qa_resp(crate::qa::QaSignal::StackmatConnected);
            }

            if let Some(limit) = global_state.solve.read(Solve::time_limit)
                && frame.1 > limit
            {
                show_time(&global_state, frame.1);
                time_end(limit, true, &global_state).await;
                tracker.end_solve();
                global_state.solve.write(|s| s.limit_armed = false);
                continue;
            }

            match tracker.frame(at, frame) {
                Some(Event::Start { at }) => {
                    if global_state.solve.update(|s| start_timer(s, Instant::from_micros(at))) {
                        global_state.timer_stop_signal.reset();
                    }
                }
                Some(Event::Stop(ms)) => time_end(ms, false, &global_state).await,
                Some(Event::Reset) => global_state.solve.update(reset_from_timer),
                None => {}
            }

            #[cfg(feature = "e2e")]
            if sim.take_ack() {
                crate::ws::send_test_ack(&global_state).await;
            }

            show_time(&global_state, frame.1);
            crate::bench!(FRAME_PUBLISH, Instant::now().as_micros().saturating_sub(at));
        }

        if tracker.connected(now) {
            continue;
        }

        // no valid frame for a while: a running solve keeps going (button can stop it)
        if let Some(ms) = tracker.running_ms(now) {
            connected = Some(false);
            if let Some(limit) = global_state.solve.read(Solve::time_limit)
                && ms > limit
            {
                show_time(&global_state, limit);
                time_end(limit, true, &global_state).await;
                tracker.end_solve();
            } else {
                show_time(&global_state, ms);
                if global_state.timer_stop_signal.try_take().is_some() {
                    time_end(ms, false, &global_state).await;
                    tracker.end_solve();
                }
            }
        } else if connected != Some(false) {
            connected = Some(false);
            global_state.timer_stop_signal.reset();
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
}

/// The timer was reset: clears an anonymous attempt, or DNFs a running one of a competitor.
#[cfg(not(feature = "timer-func"))]
fn reset_from_timer(s: &mut Solve) {
    if s.current_competitor.is_none()
        && s.penalty.is_none()
        && (s.scene == Scene::Timer || s.scene == Scene::WaitingForCompetitor)
    {
        s.scene = Scene::WaitingForCompetitor;
        s.solve_time = None;
        s.inspection_start = None;
        s.inspection_end = None;
    } else if s.current_competitor.is_some() && s.scene == Scene::Timer {
        s.scene = Scene::Finished;
        s.solve_time = Some(0);
        s.penalty = Some(-1);
        s.session_id.get_or_insert_with(crate::state::new_session_id);
        s.time_confirmed = true;
    }
}

/// e2e: frames generated from the test server's commands instead of the UART.
#[cfg(all(feature = "e2e", not(feature = "timer-func")))]
mod e2e {
    use crate::{state::GlobalState, utils::stackmat::StackmatTimerState};

    pub struct Sim {
        state: StackmatTimerState,
        target_ms: u64,
        since: embassy_time::Instant,
        ack: bool,
    }

    impl Default for Sim {
        fn default() -> Self {
            Self {
                state: StackmatTimerState::Reset,
                target_ms: 0,
                since: embassy_time::Instant::now(),
                ack: false,
            }
        }
    }

    impl Sim {
        /// Writes the current frame into `buf`, returns its length.
        pub fn frame(&mut self, global_state: &GlobalState, buf: &mut [u8; 16]) -> usize {
            if let Some((state, ms)) = global_state.e2e.stackmat_sig.try_take() {
                (self.state, self.target_ms, self.since) = (state, ms, embassy_time::Instant::now());
            }

            let ms = match self.state {
                StackmatTimerState::Unknown | StackmatTimerState::Reset => 0,
                StackmatTimerState::Running => {
                    let ms = self.since.elapsed().as_millis();
                    if ms >= self.target_ms {
                        self.state = StackmatTimerState::Stopped;
                        self.ack = true;
                    }
                    ms.min(self.target_ms)
                }
                StackmatTimerState::Stopped => self.target_ms,
            };

            buf[..8].copy_from_slice(&crate::utils::stackmat::generate_stackmat_data(
                self.state, ms,
            ));
            8
        }

        pub fn take_ack(&mut self) -> bool {
            core::mem::take(&mut self.ack)
        }
    }
}

async fn time_end(time: u64, dnf: bool, global_state: &GlobalState) {
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
    if let Some(Some(saved)) = recorded {
        saved.to_nvs(global_state.nvs).await;
    }
}
