#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(asm_experimental_arch)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::{rc::Rc, vec::Vec};
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
use utils::{logger::FkmLogger, set_brownout_detection};
use ws_framer::{WsUrl, WsUrlOwned};

mod battery;
mod bluetooth;
mod board;
mod buttons;
mod consts;
mod lcd;
mod mdns;
mod rfid;
mod stackmat;
mod state;
mod structs;
mod translations;
mod utils;
mod version;
mod ws;

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
    {
        const HEAP_SIZE: usize = 60 * 1024;

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

    let Ok(nvs) = Nvs::new_from_part_table(unsafe { board.flash.clone_unchecked() }) else {
        loop {
            log::error!("Wrong partition table! Re-flash firmware with espflash!");
            Timer::after_millis(1000).await;
        }
    };

    let global_state = Rc::new(GlobalStateInner::new(&nvs, board.aes));
    let wifi_setup_sig = Rc::new(Signal::new());
    let wifi_conn_sig = Rc::new(Signal::new());

    if let Ok(sign_key) = nvs.get::<u32>("SIGN_KEY").await {
        unsafe { crate::state::SIGN_KEY = sign_key };
    }

    spawner.must_spawn(lcd::lcd_task(
        board.lcd,
        global_state.clone(),
        wifi_setup_sig.clone(),
        board.digits_shifters.clone(),
    ));

    spawner.must_spawn(battery::battery_read_task(
        board.battery,
        board.adc1,
        global_state.clone(),
    ));
    spawner.must_spawn(buttons::buttons_task(
        global_state.clone(),
        board.button_input,
        board.buttons_shifter,
    ));
    spawner.must_spawn(stackmat::stackmat_task(
        board.uart1,
        board.stackmat_rx,
        board.digits_shifters,
        global_state.clone(),
    ));
    spawner.must_spawn(rfid::rfid_task(
        board.miso,
        board.mosi,
        board.sck,
        board.cs,
        board.spi2,
        board.spi_dma,
        global_state.clone(),
    ));

    #[cfg(feature = "qa")]
    spawner.must_spawn(qa::qa_processor(global_state.clone()));

    let mut wm_settings = esp_hal_wifimanager::WmSettings {
        wifi_panel: include_str!("panel.html"),
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
        Timer::after_millis(1000).await;
        esp_hal::system::software_reset();
    };

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
    spawner.must_spawn(ws::ws_task(
        wifi_res.sta_stack,
        ws_url,
        global_state.clone(),
        ws_sleep_sig.clone(),
        wifi_conn_sig,
    ));
    spawner.must_spawn(logger_task(global_state.clone()));

    let ble_sleep_sig = Rc::new(Signal::new());
    spawner.must_spawn(bluetooth::bluetooth_timer_task(
        wifi_res.wifi_init,
        board.bt,
        global_state.clone(),
        ble_sleep_sig.clone(),
    ));

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

        let mut tmp_logs: Vec<String> = Vec::new();
        while let Ok(msg) = utils::logger::LOGS_CHANNEL.try_receive() {
            tmp_logs.push(msg);
        }

        if ota_state() || sleep_state() {
            continue;
        }

        if !tmp_logs.is_empty() {
            tmp_logs.reverse();

            ws::send_packet(structs::TimerPacket {
                tag: None,
                data: structs::TimerPacketInner::Logs { logs: tmp_logs },
            })
            .await;
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
