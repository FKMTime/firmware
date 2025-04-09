use crate::state::{ota_state, sleep_state};
use alloc::string::String;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

const MAX_LOGS_SIZE: usize = 100;
const MIN_HEAP_REMEANING: usize = 10240;
pub static LOGS_CHANNEL: Channel<CriticalSectionRawMutex, String, MAX_LOGS_SIZE> = Channel::new();

#[cfg(feature = "release_build")]
pub const FILTER_MAX: log::LevelFilter = log::LevelFilter::Info;

#[cfg(not(feature = "release_build"))]
pub const FILTER_MAX: log::LevelFilter = log::LevelFilter::Debug;

pub struct FkmLogger;

impl FkmLogger {
    pub fn set_logger() {
        unsafe {
            _ = log::set_logger_racy(&FkmLogger);
            log::set_max_level_racy(FILTER_MAX);
        }
    }
}

impl log::Log for FkmLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let level = metadata.level();
        //let target = metadata.target();

        if level <= FILTER_MAX {
            return true;
        }
        false
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        const RESET: &str = "\u{001B}[0m";
        const RED: &str = "\u{001B}[31m";
        const GREEN: &str = "\u{001B}[32m";
        const YELLOW: &str = "\u{001B}[33m";
        const BLUE: &str = "\u{001B}[34m";
        const CYAN: &str = "\u{001B}[35m";

        let color = match record.level() {
            log::Level::Error => RED,
            log::Level::Warn => YELLOW,
            log::Level::Info => GREEN,
            log::Level::Debug => BLUE,
            log::Level::Trace => CYAN,
        };
        let reset = RESET;

        esp_println::println!("{}{} - {}{}", color, record.level(), record.args(), reset);

        #[cfg(not(any(feature = "bat_dev_lcd", feature = "qa")))]
        if !ota_state() && !sleep_state() {
            if LOGS_CHANNEL.is_full() {
                _ = LOGS_CHANNEL.try_receive();
            }

            let msg = alloc::format!("{}{} - {}{}", color, record.level(), record.args(), reset);

            // Do not send log msg to channel if heap space is too low!
            // maybe not performent but thats ok
            if esp_alloc::HEAP.free() > MIN_HEAP_REMEANING {
                _ = LOGS_CHANNEL.try_send(msg);
            } else {
                // clear logs channel if heap space is too low
                LOGS_CHANNEL.clear();
            }
        }
    }

    fn flush(&self) {}
}
