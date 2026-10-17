#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// errors carry fixed-size server text (no heap), so large `Err` variants are intended
#![allow(clippy::result_large_err)]

#[cfg(all(feature = "bat_dev_lcd", not(feature = "v3")))]
compile_error!("feature `bat_dev_lcd` is a v3 battery debug screen");

extern crate alloc;
use crate::consts::NVS_SIGN_KEY;
use board::Board;
use consts::LOG_SEND_INTERVAL_MS;
use embassy_executor::Spawner;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use esp_backtrace as _;
use esp_hal_wifimanager::{Nvs, WIFI_NVS_KEY};
use esp_storage::FlashStorage;
use state::{GlobalState, GlobalStateInner, SavedGlobalState, Scene};
use structs::ConnSettings;
use utils::{logger::FkmLogger, set_brownout_detection, spawn_task};

mod bench;
mod bluetooth;
mod board;
mod buttons;
mod consts;
mod mdns;
mod power;
mod rfid;
mod stackmat;
mod state;
mod structs;
mod translations;
mod utils;
mod version;
mod watchdog;
mod ws;

#[cfg(feature = "v3")]
mod battery_v3;
#[cfg(feature = "v3")]
mod lcd_v3;

#[cfg(feature = "v4")]
mod battery_v4;
#[cfg(feature = "v4")]
mod lcd_v4;

#[cfg(feature = "qa")]
mod qa;

pub fn custom_rng(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    for chunk in buf.chunks_mut(4) {
        let random_u32 = unsafe { &*esp_hal::peripherals::RNG::PTR }
            .data()
            .read()
            .bits();

        let len = chunk.len();
        chunk[..].copy_from_slice(&random_u32.to_be_bytes()[..len]);
    }

    Ok(())
}
getrandom::register_custom_getrandom!(custom_rng);
esp_bootloader_esp_idf::esp_app_desc!();

/// Uptime after which a crash no longer counts as part of a crash loop.
const HEALTHY_AFTER_SECS: u64 = 60;
/// Error log flash writes are throttled (repeating errors must not wear the flash).
const ERROR_LOG_SAVE_INTERVAL: embassy_time::Duration = embassy_time::Duration::from_secs(60);

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    utils::stack::paint();
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config = config.with_cpu_clock(esp_hal::clock::CpuClock::_80MHz);

        config
    });

    // wifimanager 0.5+ keeps its signals/buffers in statics instead of the heap
    esp_alloc::heap_allocator!(size: 112 * 1024);

    set_brownout_detection(false);
    let board = Board::init(peripherals);
    esp_rtos::start(board.timg1.timer0, board.sw_interrupt);
    FkmLogger::set_logger();
    spawn_task(
        &spawner,
        "watchdog::watchdog_task",
        watchdog::watchdog_task(board.timg0.wdt),
    );

    log::info!("Version: {}", version::VERSION);
    log::info!("Hardware Rev: {}", version::HW_VER);
    log::info!("Firmware: {}", version::FIRMWARE);

    #[cfg(feature = "e2e")]
    log::info!("This firmware is in E2E mode!");

    #[cfg(feature = "qa")]
    log::info!("This firmware is in QA mode!");

    let Ok(nvs) = Nvs::new(unsafe { board.flash.clone_unchecked() }) else {
        utils::error_log::add_error(utils::error_log::codes::WRONG_PARTITION_TABLE);
        #[cfg(feature = "v3")]
        lcd_v3::lcd_show_critical_code(board.lcd, utils::error_log::codes::WRONG_PARTITION_TABLE)
            .await;

        #[cfg(feature = "v4")]
        lcd_v4::lcd_show_critical_code(
            board.i2c,
            board.display_rst,
            utils::error_log::codes::WRONG_PARTITION_TABLE,
        )
        .await;

        loop {
            log::error!("Wrong partition table! Re-flash firmware with espflash!");
            Timer::after_millis(1000).await;
        }
    };

    let reason = esp_hal::rtc_cntl::reset_reason(esp_hal::system::Cpu::ProCpu);
    let wake_reason = esp_hal::rtc_cntl::wakeup_cause();
    log::info!("Wake reason: {:?} {:?}", reason, wake_reason);

    utils::error_log::load_error_log(nvs).await;
    utils::backtrace_store::report_saved_crash();

    static GLOBAL_STATE: static_cell::StaticCell<GlobalStateInner> = static_cell::StaticCell::new();
    let global_state: GlobalState = GLOBAL_STATE.init(GlobalStateInner::new(nvs, board.aes));
    static WIFI_SETUP_SIG: esp_hal_wifimanager::ProvisioningSignal = Signal::new();
    static WIFI_CONN_SIG: esp_hal_wifimanager::ConnSignal = Signal::new();

    if let Ok(sign_key) = nvs.get::<u32>(&NVS_SIGN_KEY).await {
        crate::state::SETTINGS.write(|s| s.sign_key = sign_key);
    }
    #[cfg(feature = "v4")]
    if let Ok(saved_volume) = nvs.get::<u8>(&crate::consts::NVS_BUZZER_VOLUME).await {
        if (crate::consts::BUZZER_VOLUME_MIN..=crate::consts::BUZZER_VOLUME_MAX)
            .contains(&saved_volume)
        {
            crate::state::SETTINGS.write(|s| s.buzzer_volume = saved_volume);
        } else {
            log::warn!(
                "Ignoring invalid saved buzzer volume: {} (valid range {}..={})",
                saved_volume,
                crate::consts::BUZZER_VOLUME_MIN,
                crate::consts::BUZZER_VOLUME_MAX
            );
        }
    }

    #[cfg(feature = "v3")]
    spawn_task(
        &spawner,
        "lcd_v3::lcd_task",
        lcd_v3::lcd_task(board.lcd, global_state, &WIFI_SETUP_SIG),
    );
    #[cfg(feature = "v4")]
    spawn_task(
        &spawner,
        "lcd_v4::lcd_task",
        lcd_v4::lcd_task(board.i2c, board.display_rst, global_state, &WIFI_SETUP_SIG),
    );

    #[cfg(feature = "v3")]
    spawn_task(
        &spawner,
        "battery_v3::battery_read_task",
        battery_v3::battery_read_task(board.battery, board.adc1, global_state),
    );
    #[cfg(feature = "v4")]
    spawn_task(
        &spawner,
        "battery_v4::battery_read_task",
        battery_v4::battery_read_task(board.i2c, global_state),
    );

    spawn_task(
        &spawner,
        "buttons::buttons_task",
        buttons::buttons_task(
            global_state,
            #[cfg(feature = "v4")]
            board.buttons,
            #[cfg(feature = "v3")]
            board.button_input,
            #[cfg(feature = "v3")]
            board.buttons_shifter,
        ),
    );
    spawn_task(
        &spawner,
        "stackmat::stackmat_task",
        stackmat::stackmat_task(
            #[cfg(not(feature = "timer-func"))]
            board.uart1,
            #[cfg(not(feature = "timer-func"))]
            board.stackmat_rx,
            global_state,
            #[cfg(feature = "timer-func")]
            board.pads,
        ),
    );
    spawn_task(
        &spawner,
        "rfid::rfid_task",
        rfid::rfid_task(
            #[cfg(feature = "v4")]
            board.i2c,
            #[cfg(feature = "v4")]
            board.buzzer,
            #[cfg(feature = "v3")]
            board.miso,
            #[cfg(feature = "v3")]
            board.mosi,
            #[cfg(feature = "v3")]
            board.sck,
            #[cfg(feature = "v3")]
            board.cs,
            #[cfg(feature = "v3")]
            board.spi2,
            #[cfg(feature = "v3")]
            board.spi_dma,
            global_state,
        ),
    );

    #[cfg(feature = "qa")]
    spawn_task(&spawner, "qa::qa_processor", qa::qa_processor(global_state));

    let mut wm_settings = esp_hal_wifimanager::WmSettings {
        wifi_panel: esp_hal_wifimanager::include_panel!("src/panel.html"),
        wifi_conn_signal: Some(&WIFI_CONN_SIG),
        provisioning_signal: Some(&WIFI_SETUP_SIG),
        ..Default::default()
    };

    wm_settings.ssid.clear();
    _ = core::fmt::write(
        &mut wm_settings.ssid,
        format_args!("FKM-{:X}", crate::utils::get_efuse_u32()),
    );

    // mark ota as valid
    {
        if let Ok(mut ota) =
            esp_hal_ota::Ota::new(FlashStorage::new(unsafe { board.flash.clone_unchecked() }))
        {
            let res = ota.mark_app_valid();
            if let Err(e) = res {
                log::error!("Ota mark app valid failed: {e:?}");
                utils::error_log::add_error(utils::error_log::codes::OTA_MARK_VALID_FAILED);
            }
        }
    }

    Timer::after_millis(500).await;
    let wifi_res = esp_hal_wifimanager::init_wm(
        wm_settings,
        &spawner,
        #[cfg(feature = "qa")]
        None,
        #[cfg(not(feature = "qa"))]
        Some(nvs),
        board.wifi,
        unsafe { board.bt.clone_unchecked() },
    )
    .await;

    let Ok(wifi_res) = wifi_res else {
        log::error!("WifiManager failed!!! Restarting in 1s!");
        utils::error_log::add_error(utils::error_log::codes::WIFI_MANAGER_FAILED);
        utils::error_log::save_error_log(nvs).await;
        Timer::after_millis(1000).await;
        esp_hal::system::software_reset();
    };

    {
        global_state
            .status
            .update(|s| s.wifi_connected = Some(true));
    }

    #[cfg(feature = "qa")]
    crate::qa::send_qa_resp(crate::qa::QaSignal::WifiSetup);

    let conn_settings: ConnSettings = wifi_res
        .data()
        .and_then(|d| serde_json::from_str(d).ok())
        .unwrap_or_default();

    let mut parse_retry_count = 0;
    let ws_url = loop {
        let url = if conn_settings.mdns || conn_settings.ws_url.is_none() || parse_retry_count > 0 {
            log::info!("Starting mdns lookup...");
            global_state.solve.update(|s| s.scene = Scene::MdnsWait);
            let mdns_res = mdns::mdns_query(wifi_res.stack()).await;
            log::info!("Mdns result: {mdns_res:?}");

            mdns_res
        } else {
            // a url longer than 255 bytes counts as unparsable
            conn_settings
                .ws_url
                .as_deref()
                .and_then(|u| heapless::String::try_from(u).ok())
                .unwrap_or_default()
        };

        match ws_framer::WsUrl::parse(&url) {
            Some(_) => break url,
            None => {
                parse_retry_count += 1;
                log::error!("Mdns parse failed! Retry ({parse_retry_count})..");
                Timer::after_millis(1000).await;
                if parse_retry_count > 3 {
                    log::error!("Cannot parse wsurl! Reseting wifi configuration!");
                    utils::error_log::add_error(utils::error_log::codes::MDNS_WS_URL_PARSE_FAILED);
                    utils::error_log::save_error_log(nvs).await;
                    _ = nvs.delete(&WIFI_NVS_KEY).await;
                    Timer::after_millis(1000).await;

                    esp_hal::system::software_reset();
                }

                continue;
            }
        }
    };

    spawn_task(
        &spawner,
        "ws::ws_task",
        ws::ws_task(wifi_res.stack(), ws_url, global_state, &WIFI_CONN_SIG),
    );
    spawn_task(&spawner, "logger_task", logger_task(global_state));

    spawn_task(
        &spawner,
        "bluetooth::bluetooth_timer_task",
        bluetooth::bluetooth_timer_task(board.bt, global_state),
    );

    set_brownout_detection(true);
    global_state
        .solve
        .update(|s| s.scene = Scene::WaitingForCompetitor);
    spawn_task(
        &spawner,
        "restore_saved_state_task",
        restore_saved_state_task(global_state, nvs),
    );

    let mut power = power::Supervisor::new(&wifi_res);
    let boot = Instant::now();
    let mut healthy = false;
    let mut last_error_log_save: Option<Instant> = None;
    loop {
        Timer::after_millis(100).await;
        watchdog::heartbeat(watchdog::TaskId::Main);
        if !healthy && boot.elapsed().as_secs() >= HEALTHY_AFTER_SECS {
            utils::backtrace_store::mark_healthy();
            healthy = true;
        }

        if utils::error_log::is_save_ready()
            && last_error_log_save.is_none_or(|t| t.elapsed() >= ERROR_LOG_SAVE_INTERVAL)
        {
            // cleared first so errors added while saving trigger the next save
            utils::error_log::clear_save_ready();
            utils::error_log::save_error_log(nvs).await;
            last_error_log_save = Some(Instant::now());
        }

        power.tick();
    }
}

/// Restores an unfinished solve; waits for the server time, so it can't run in the main loop.
#[embassy_executor::task]
async fn restore_saved_state_task(global_state: GlobalState, nvs: &'static Nvs) {
    if let Some(saved_state) = SavedGlobalState::from_nvs(nvs).await {
        global_state.solve.update(|s| {
            // a new solve may have started while waiting for the server time
            if s.scene == Scene::WaitingForCompetitor {
                s.restore(saved_state);
            }
        });
    }
}

#[embassy_executor::task]
async fn logger_task(global_state: GlobalState) {
    let mut heap_start = Instant::now();

    loop {
        Timer::after_millis(LOG_SEND_INTERVAL_MS).await;

        if crate::ws::ota_active() || crate::power::is_sleeping() {
            continue;
        }

        let current_time = global_state.timer.try_get();
        let logs = global_state.solve.read(|s| {
            crate::utils::logger::take_ready(&crate::utils::logger::LogsHeader {
                current_time,
                inspection_time: s.inspection_ms(),
                competitor: s.current_competitor,
                group_id: s.solve_group.as_ref().map(|g| g.group_id.as_str()),
                session_id: s.session_id.as_deref(),
            })
        });

        if let Some(logs) = logs {
            _ = ws::send_logs(logs);
        }

        if heap_start.elapsed().as_millis() >= consts::PRINT_HEAP_INTERVAL_MS {
            let stats = esp_alloc::HEAP.stats();
            log::info!(
                "Heap: used {} max {} of {} bytes",
                stats.current_usage,
                stats.max_usage,
                stats.size
            );
            let (stack_used, stack_size) = utils::stack::high_water();
            log::info!("Stack: max {stack_used} of {stack_size} bytes");
            heap_start = Instant::now();
        }
    }
}
