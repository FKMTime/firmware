#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

extern crate alloc;
use crate::consts::NVS_SIGN_KEY;
use alloc::rc::Rc;
use alloc::string::ToString;
use board::Board;
use consts::LOG_SEND_INTERVAL_MS;
use embassy_executor::Spawner;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use esp_backtrace as _;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal_wifimanager::{Nvs, WIFI_NVS_KEY};
use esp_storage::FlashStorage;
use state::{GlobalState, GlobalStateInner, SavedGlobalState, Scene, ota_state, sleep_state};
use structs::ConnSettings;
use utils::{logger::FkmLogger, set_brownout_detection, spawn_task};
use ws_framer::{WsUrl, WsUrlOwned};

mod bluetooth;
mod board;
mod buttons;
mod consts;
mod mdns;
mod rfid;
mod stackmat;
mod state;
mod structs;
mod translations;
mod utils;
mod version;
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

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config = config.with_cpu_clock(esp_hal::clock::CpuClock::_80MHz);

        config
    });

    esp_alloc::heap_allocator!(size: 120 * 1024);
    /*
    {
        const HEAP_SIZE: usize = 8 * 1024;

        #[unsafe(link_section = ".dram2_uninit")]
        static mut HEAP2: core::mem::MaybeUninit<[u8; HEAP_SIZE]> =
            core::mem::MaybeUninit::uninit();

        #[allow(static_mut_refs)]
        unsafe {
            esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
                HEAP2.as_mut_ptr() as *mut u8,
                core::mem::size_of_val(&*core::ptr::addr_of!(HEAP2)),
                esp_alloc::MemoryCapability::Internal.into(),
            ));
        }
    }
    */

    set_brownout_detection(false);
    let board = Board::init(peripherals);
    let software_interrupt = SoftwareInterruptControl::new(board.sw_interrupt);
    esp_rtos::start(board.timg1.timer0, software_interrupt.software_interrupt0);
    FkmLogger::set_logger();

    log::info!("Version: {}", version::VERSION);
    log::info!("Hardware Rev: {}", version::HW_VER);
    log::info!("Firmware: {}", version::FIRMWARE);

    #[cfg(feature = "e2e")]
    log::info!("This firmware is in E2E mode!");

    #[cfg(feature = "qa")]
    log::info!("This firmware is in QA mode!");

    #[cfg(feature = "release_build")]
    crate::utils::backtrace_store::verify_panic_flag();

    let Ok(nvs) = Nvs::new_from_part_table(unsafe { board.flash.clone_unchecked() }) else {
        let mut error_logged = false;
        loop {
            log::error!("Wrong partition table! Re-flash firmware with espflash!");
            if !error_logged {
                utils::error_log::add_error(utils::error_log::codes::WRONG_PARTITION_TABLE).await;
                error_logged = true;
            }
            Timer::after_millis(1000).await;
        }
    };

    let reason = esp_hal::rtc_cntl::reset_reason(esp_hal::system::Cpu::ProCpu);
    let wake_reason = esp_hal::rtc_cntl::wakeup_cause();
    log::info!("Wake reason: {:?} {:?}", reason, wake_reason);

    utils::error_log::load_error_log(&nvs).await;

    let global_state = Rc::new(GlobalStateInner::new(&nvs, board.aes));
    let wifi_setup_sig = Rc::new(Signal::new());
    let wifi_conn_sig = Rc::new(Signal::new());

    if let Ok(sign_key) = nvs.get::<u32>(NVS_SIGN_KEY).await {
        unsafe { crate::state::SIGN_KEY = sign_key };
    }
    #[cfg(feature = "v4")]
    if let Ok(saved_volume) = nvs.get::<u8>(crate::consts::NVS_BUZZER_VOLUME).await {
        if (crate::consts::BUZZER_VOLUME_MIN..=crate::consts::BUZZER_VOLUME_MAX)
            .contains(&saved_volume)
        {
            crate::state::set_buzzer_volume(saved_volume);
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
        lcd_v3::lcd_task(
            board.lcd,
            global_state.clone(),
            wifi_setup_sig.clone(),
            board.digits_shifters.clone(),
        ),
    );
    #[cfg(feature = "v4")]
    spawn_task(
        &spawner,
        "lcd_v4::lcd_task",
        lcd_v4::lcd_task(
            board.i2c.clone(),
            board.display_rst,
            global_state.clone(),
            wifi_setup_sig.clone(),
        ),
    );

    #[cfg(feature = "v3")]
    spawn_task(
        &spawner,
        "battery_v3::battery_read_task",
        battery_v3::battery_read_task(board.battery, board.adc1, global_state.clone()),
    );
    #[cfg(feature = "v4")]
    spawn_task(
        &spawner,
        "battery_v4::battery_read_task",
        battery_v4::battery_read_task(board.i2c.clone(), global_state.clone()),
    );

    spawn_task(
        &spawner,
        "buttons::buttons_task",
        buttons::buttons_task(
            global_state.clone(),
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
            board.uart1,
            board.stackmat_rx,
            #[cfg(feature = "v3")]
            board.digits_shifters,
            global_state.clone(),
        ),
    );
    spawn_task(
        &spawner,
        "rfid::rfid_task",
        rfid::rfid_task(
            #[cfg(feature = "v4")]
            board.i2c.clone(),
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
            global_state.clone(),
        ),
    );

    #[cfg(feature = "qa")]
    spawn_task(
        &spawner,
        "qa::qa_processor",
        qa::qa_processor(global_state.clone()),
    );

    let mut wm_settings = esp_hal_wifimanager::WmSettings {
        wifi_panel: esp_hal_wifimanager::include_minified!("src/panel.html"),
        wifi_conn_signal: Some(wifi_conn_sig.clone()),
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
            let res = ota.ota_mark_app_valid();
            if let Err(e) = res {
                log::error!("Ota mark app valid failed: {e:?}");
                utils::error_log::add_error(utils::error_log::codes::OTA_MARK_VALID_FAILED).await;
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
        Some(&nvs),
        board.wifi,
        unsafe { board.bt.clone_unchecked() },
        Some(wifi_setup_sig),
    )
    .await;

    let Ok(mut wifi_res) = wifi_res else {
        log::error!("WifiManager failed!!! Restarting in 1s!");
        utils::error_log::add_error(utils::error_log::codes::WIFI_MANAGER_FAILED).await;
        utils::error_log::save_error_log(&nvs).await;
        Timer::after_millis(1000).await;
        esp_hal::system::software_reset();
    };

    {
        global_state.state.lock().await.wifi_connected = Some(true);
    }

    #[cfg(feature = "qa")]
    crate::qa::send_qa_resp(crate::qa::QaSignal::WifiSetup);

    let conn_settings: ConnSettings = wifi_res
        .data
        .take()
        .and_then(|d| serde_json::from_value(d).ok())
        .unwrap_or_default();

    let mut parse_retry_count = 0;
    let ws_url = loop {
        let url = if conn_settings.mdns || conn_settings.ws_url.is_none() || parse_retry_count > 0 {
            log::info!("Starting mdns lookup...");
            global_state.state.lock().await.scene = Scene::MdnsWait;
            let mdns_res = mdns::mdns_query(wifi_res.sta_stack).await;
            log::info!("Mdns result: {mdns_res:?}");

            mdns_res.to_string()
        } else {
            conn_settings.ws_url.clone().unwrap_or_default()
        };

        let ws_url = WsUrl::from_str(&url);
        match ws_url {
            Some(ws_url) => break WsUrlOwned::new(&ws_url),
            None => {
                parse_retry_count += 1;
                log::error!("Mdns parse failed! Retry ({parse_retry_count})..");
                Timer::after_millis(1000).await;
                if parse_retry_count > 3 {
                    log::error!("Cannot parse wsurl! Reseting wifi configuration!");
                    utils::error_log::add_error(utils::error_log::codes::MDNS_WS_URL_PARSE_FAILED)
                        .await;
                    utils::error_log::save_error_log(&nvs).await;
                    _ = nvs.delete(WIFI_NVS_KEY).await;
                    Timer::after_millis(1000).await;

                    esp_hal::system::software_reset();
                }

                continue;
            }
        }
    };

    utils::backtrace_store::read_saved_backtrace().await;

    let ws_sleep_sig = Rc::new(Signal::new());
    spawn_task(
        &spawner,
        "ws::ws_task",
        ws::ws_task(
            wifi_res.sta_stack,
            ws_url,
            global_state.clone(),
            ws_sleep_sig.clone(),
            wifi_conn_sig,
        ),
    );
    spawn_task(&spawner, "logger_task", logger_task(global_state.clone()));

    let ble_sleep_sig = Rc::new(Signal::new());
    spawn_task(
        &spawner,
        "bluetooth::bluetooth_timer_task",
        bluetooth::bluetooth_timer_task(board.bt, global_state.clone(), ble_sleep_sig.clone()),
    );

    set_brownout_detection(true);
    global_state.state.lock().await.scene = Scene::WaitingForCompetitor;
    if let Some(saved_state) = SavedGlobalState::from_nvs(&nvs).await {
        global_state
            .state
            .lock()
            .await
            .parse_saved_state(saved_state);
    }

    let mut last_sleep = false;
    loop {
        Timer::after_millis(100).await;
        if utils::error_log::is_save_ready() {
            utils::error_log::save_error_log(&nvs).await;
            utils::error_log::clear_save_ready();
        }

        if sleep_state() != last_sleep {
            last_sleep = sleep_state();
            ws_sleep_sig.signal(last_sleep);
            ble_sleep_sig.signal(last_sleep);

            match last_sleep {
                true => wifi_res.stop_radio(),
                false => wifi_res.restart_radio(),
            }
        }
    }
}

#[embassy_executor::task]
async fn logger_task(global_state: GlobalState) {
    #[cfg(not(feature = "release_build"))]
    let mut heap_start = Instant::now();

    loop {
        Timer::after_millis(LOG_SEND_INTERVAL_MS).await;

        if ota_state() || sleep_state() {
            continue;
        }

        #[allow(static_mut_refs)]
        let logs_vec = unsafe {
            crate::utils::logger::LOGS_WRITER.get_vec(Some(crate::stackmat::CURRENT_TIME))
        };

        if !logs_vec.is_empty() {
            ws::send_frame(ws_framer::WsFrameOwned::Binary(logs_vec)).await;
        }

        #[cfg(not(feature = "release_build"))]
        if (Instant::now() - heap_start).as_millis() >= consts::PRINT_HEAP_INTERVAL_MS {
            if global_state.state.lock().await.server_connected == Some(true) {
                log::info!("{}", esp_alloc::HEAP.stats());
            }

            heap_start = Instant::now();
        }
    }
}
