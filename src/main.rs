#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(asm_experimental_arch)]

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::{rc::Rc, vec::Vec};
use board::Board;
use consts::{LOG_SEND_INTERVAL_MS, PRINT_HEAP_INTERVAL_MS};
use embassy_executor::Spawner;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use esp_backtrace as _;
use esp_hal_wifimanager::{Nvs, WIFI_NVS_KEY};
use esp_storage::FlashStorage;
use state::{ota_state, sleep_state, GlobalState, GlobalStateInner, SavedGlobalState, Scene};
use structs::ConnSettings;
use utils::{logger::FkmLogger, set_brownout_detection};
use ws_framer::{WsUrl, WsUrlOwned};

mod battery;
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

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();

        #[cfg(feature = "esp32")]
        {
            config = config.with_cpu_clock(esp_hal::clock::CpuClock::_80MHz);
        }

        #[cfg(feature = "esp32c3")]
        {
            config = config.with_cpu_clock(esp_hal::clock::CpuClock::_80MHz);
        }

        config
    });

    set_brownout_detection(false);
    let board = Board::init(peripherals).expect("Board init error");
    {
        if let Ok(mut ota) = esp_hal_ota::Ota::new(FlashStorage::new()) {
            let res = ota.ota_mark_app_valid();
            if let Err(e) = res {
                log::error!("Ota mark app valid failed: {e:?}");
            }
        }
    }

    // second heap init
    {
        #[cfg(feature = "esp32")]
        const HEAP_SIZE: usize = 90 * 1024;

        #[cfg(not(feature = "esp32"))]
        const HEAP_SIZE: usize = 60 * 1024;

        #[link_section = ".dram2_uninit"]
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

    FkmLogger::set_logger();
    esp_hal_embassy::init(board.timg1.timer0);

    #[cfg(feature = "e2e")]
    log::info!("This firmware is E2E! (HIL TESTING)");

    let nvs = Nvs::new_from_part_table().expect("Wrong partition configuration!");
    let global_state = Rc::new(GlobalStateInner::new(&nvs));
    let wifi_setup_sig = Rc::new(Signal::new());

    _ = spawner.spawn(lcd::lcd_task(
        board.lcd,
        global_state.clone(),
        wifi_setup_sig.clone(),
        board.digits_shifters.clone(),
    ));

    _ = spawner.spawn(battery::battery_read_task(
        board.battery,
        board.adc1,
        global_state.clone(),
    ));
    _ = spawner.spawn(buttons::buttons_task(
        global_state.clone(),
        board.button_input,
        #[cfg(feature = "esp32c3")]
        board.buttons_shifter,
    ));
    _ = spawner.spawn(stackmat::stackmat_task(
        board.uart1,
        board.stackmat_rx,
        board.digits_shifters,
        global_state.clone(),
    ));
    _ = spawner.spawn(rfid::rfid_task(
        board.miso,
        board.mosi,
        board.sck,
        board.cs,
        board.spi2,
        board.spi_dma,
        global_state.clone(),
    ));

    let mut wm_settings = esp_hal_wifimanager::WmSettings::default();
    wm_settings.ssid.clear();
    _ = core::fmt::write(
        &mut wm_settings.ssid,
        format_args!("FKM-{:X}", crate::utils::get_efuse_u32()),
    );

    #[cfg(feature = "esp32")]
    {
        wm_settings.esp_restart_after_connection = true;
    }

    let wifi_res = esp_hal_wifimanager::init_wm(
        wm_settings,
        &spawner,
        &nvs,
        board.rng,
        board.timg0.timer0,
        board.radio_clk,
        board.wifi,
        board.bt,
        Some(wifi_setup_sig),
    )
    .await;

    let Ok(mut wifi_res) = wifi_res else {
        log::error!("WifiManager failed!!! Restarting in 1s!");
        Timer::after_millis(1000).await;
        esp_hal::system::software_reset();
    };

    let conn_settings: ConnSettings = wifi_res
        .data
        .take()
        .and_then(|d| serde_json::from_value(d).ok())
        .unwrap_or_default();

    let mut parse_retry_count = 0;
    let ws_url = loop {
        let url = if conn_settings.mdns || conn_settings.ws_url.is_none() || parse_retry_count > 0 {
            log::info!("Start mdns lookup...");
            global_state.state.lock().await.scene = Scene::MdnsWait;
            let mdns_res = mdns::mdns_query(wifi_res.sta_stack).await;
            log::info!("Mdns result: {:?}", mdns_res);

            mdns_res.to_string()
        } else {
            conn_settings.ws_url.clone().expect("")
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
                    _ = nvs.invalidate_key(WIFI_NVS_KEY).await;
                    Timer::after_millis(1000).await;

                    esp_hal::system::software_reset();
                }

                continue;
            }
        }
    };

    utils::backtrace_store::read_saved_backtrace().await;

    let ws_sleep_sig = Rc::new(Signal::new());
    _ = spawner.spawn(ws::ws_task(
        wifi_res.sta_stack,
        ws_url,
        global_state.clone(),
        ws_sleep_sig.clone(),
    ));
    _ = spawner.spawn(logger_task(global_state.clone()));

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

            match last_sleep {
                true => wifi_res.stop_radio(),
                false => wifi_res.restart_radio(),
            }
        }
    }
}

#[embassy_executor::task]
async fn logger_task(global_state: GlobalState) {
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

        if (Instant::now() - heap_start).as_millis() >= PRINT_HEAP_INTERVAL_MS {
            if global_state.state.lock().await.server_connected == Some(true) {
                log::info!("{}", esp_alloc::HEAP.stats());
            }

            heap_start = Instant::now();
        }
    }
}
