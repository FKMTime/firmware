//! Task watchdog: the TIMG0 hardware watchdog is fed only while every armed task is alive.
//!
//! A task is armed by its first [`heartbeat`] and must then beat at least every
//! [`TASK_TIMEOUT_SECS`]. A stuck executor stops the feeding task itself, so the hardware
//! watchdog resets the chip after [`HW_TIMEOUT_SECS`].

use embassy_time::{Instant, Timer};
use esp_hal::{
    peripherals::TIMG0,
    timer::timg::{MwdtStage, Wdt},
};
use portable_atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug)]
pub enum TaskId {
    Main,
    Ws,
    Lcd,
    Buttons,
    Stackmat,
}

const TASKS: [TaskId; 5] = [
    TaskId::Main,
    TaskId::Ws,
    TaskId::Lcd,
    TaskId::Buttons,
    TaskId::Stackmat,
];
const TASK_TIMEOUT_SECS: u32 = 60;
const HW_TIMEOUT_SECS: u64 = 10;

#[allow(clippy::declare_interior_mutable_const)]
const NOT_ARMED: AtomicU32 = AtomicU32::new(0);
/// Uptime in seconds + 1 of each task's last heartbeat (0 = not armed).
static LAST_BEAT: [AtomicU32; TASKS.len()] = [NOT_ARMED; TASKS.len()];

fn now() -> u32 {
    (Instant::now().as_secs() as u32).saturating_add(1)
}

/// Marks `task` alive (and arms it on the first call).
pub fn heartbeat(task: TaskId) {
    LAST_BEAT[task as usize].store(now(), Ordering::Relaxed);
}

/// The first armed task that missed its deadline.
fn stale_task() -> Option<TaskId> {
    let now = now();
    // tasks are allowed to idle while sleeping; they re-arm on their next heartbeat
    let sleeping = crate::power::is_sleeping();

    TASKS.into_iter().find(|&task| {
        let beat = &LAST_BEAT[task as usize];
        if sleeping && !matches!(task, TaskId::Main) {
            beat.store(0, Ordering::Relaxed);
            return false;
        }

        let last = beat.load(Ordering::Relaxed);
        last != 0 && now.saturating_sub(last) > TASK_TIMEOUT_SECS
    })
}

#[embassy_executor::task]
pub async fn watchdog_task(mut wdt: Wdt<TIMG0<'static>>) {
    wdt.set_timeout(
        MwdtStage::Stage0,
        esp_hal::time::Duration::from_secs(HW_TIMEOUT_SECS),
    );
    wdt.enable();

    loop {
        Timer::after_secs(1).await;
        match stale_task() {
            None => wdt.feed(),
            Some(task) => {
                log::error!("Task {task:?} stopped responding! Resetting");
                crate::utils::backtrace_store::record_watchdog(task as u8);
                esp_hal::system::software_reset();
            }
        }
    }
}
