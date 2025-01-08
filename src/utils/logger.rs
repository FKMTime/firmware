use crate::state::get_ota_state;
use alloc::{string::String, vec::Vec};
use core::cell::OnceCell;

pub const FILTER_MAX: log::LevelFilter = log::LevelFilter::Debug;
pub static mut GLOBAL_LOGS: OnceCell<Vec<String>> = OnceCell::new();

pub fn init_global_logs_store() {
    unsafe {
        GLOBAL_LOGS
            .set(Vec::new())
            .expect("Failed to set GLOBAL_LOGS");
    }
}

pub struct FkmLogger;

impl FkmLogger {
    pub fn set_logger() {
        unsafe {
            log::set_logger_racy(&FkmLogger).unwrap();
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
        if !self.enabled(&record.metadata()) {
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

        if !get_ota_state() {
            unsafe {
                if let Some(logs_buf) = GLOBAL_LOGS.get_mut() {
                    logs_buf.push(alloc::format!(
                        "{}{} - {}{}",
                        color,
                        record.level(),
                        record.args(),
                        reset
                    ));
                }
            }
        }
    }

    fn flush(&self) {}
}
