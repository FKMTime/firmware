#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(asm_experimental_arch)]

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::{rc::Rc, vec::Vec};
use board::Board;
use consts::LOG_SEND_INTERVAL_MS;
use embassy_executor::Spawner;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use esp_backtrace as _;
use esp_hal_wifimanager::{Nvs, WIFI_NVS_KEY};
use esp_storage::FlashStorage;
use state::{GlobalState, GlobalStateInner, SavedGlobalState, Scene, ota_state, sleep_state};
use structs::ConnSettings;
use utils::{logger::FkmLogger, set_brownout_detection};
use ws_framer::{WsUrl, WsUrlOwned};

#[cfg(feature = "esp_now")]
use esp_wifi::esp_now::{EspNowManager, EspNowSender};

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

#[cfg(feature = "esp_now")]
const ESP_NOW_DST: &[u8; 6] = &[156, 158, 110, 52, 70, 200];
#[cfg(feature = "esp_now")]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal_embassy::main]
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
    let board = Board::init(peripherals).expect("Board init error");
    esp_hal_embassy::init(board.timg1.timer0);
    FkmLogger::set_logger();

    log::info!("Version: {}", version::VERSION);
    log::info!("Hardware Rev: {}", version::HW_VER);
    log::info!("Firmware: {}", version::FIRMWARE);

    #[cfg(feature = "e2e")]
    log::info!("This firmware is in E2E mode!");

    #[cfg(feature = "qa")]
    log::info!("This firmware is in QA mode!");

    let nvs = Nvs::new_from_part_table().expect("Wrong partition configuration!");
    let global_state = Rc::new(GlobalStateInner::new(&nvs));
    let wifi_setup_sig = Rc::new(Signal::new());

    // TODO: add error handling here
    let mut sign_key = [0; 4];
    if nvs.get_key(b"SIGN_KEY", &mut sign_key).await.is_ok() {
        unsafe {
            crate::state::SIGN_KEY =
                u32::from_be_bytes(sign_key.try_into().expect("Cannot fail")) >> 1;
        }
    } else {
        _ = getrandom::getrandom(&mut sign_key);
        _ = nvs.append_key(b"SIGN_KEY", &sign_key).await;
        unsafe {
            crate::state::SIGN_KEY =
                u32::from_be_bytes(sign_key.try_into().expect("Cannot fail")) >> 1;
        }
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
        ..Default::default()
    };

    wm_settings.ssid.clear();
    _ = core::fmt::write(
        &mut wm_settings.ssid,
        format_args!("FKM-{:X}", crate::utils::get_efuse_u32()),
    );

    // mark ota as valid
    {
        if let Ok(mut ota) = esp_hal_ota::Ota::new(FlashStorage::new()) {
            let res = ota.ota_mark_app_valid();
            if let Err(e) = res {
                log::error!("Ota mark app valid failed: {e:?}");
            }
        }
    }

    #[cfg(feature = "esp_now")]
    {
        let init = &*mk_static!(
            esp_wifi::EspWifiController<'static>,
            esp_wifi::init(board.timg0.timer0, board.rng.clone(), board.radio_clk).unwrap()
        );

        let (manager, tx, mut rx) = esp_wifi::esp_now::EspNow::new(&init, board.wifi)
            .unwrap()
            .split();

        _ = manager.set_power_saving(esp_wifi::config::PowerSaveMode::None);
        //_ = manager.set_rate(esp_wifi::esp_now::WifiPhyRate::RateMax);
        _ = manager.set_pmk(&[69, 4, 2, 0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

        spawner.must_spawn(bc_test_task(tx, manager));
        let mut last_counter = 0;
        loop {
            let recv = rx.receive_async().await;
            log::info!("ESP_NOW frame recv: {:?}", recv.info);
            log::info!("{:?}", recv.data());

            if &recv.info.dst_address == ESP_NOW_DST {
                if recv.data().len() == 16 {
                    let counter = u128::from_be_bytes(recv.data().try_into().unwrap());
                    if counter - last_counter == 0 {
                        log::warn!("First packet?");
                    } else if counter - last_counter > 1 {
                        log::error!("PACKET LOST!");
                    }

                    last_counter = counter;
                }
            }
        }
    }

    #[cfg(not(feature = "esp_now"))]
    {
        let wifi_res = esp_hal_wifimanager::init_wm(
            wm_settings,
            &spawner,
            #[cfg(feature = "qa")]
            None,
            #[cfg(not(feature = "qa"))]
            Some(&nvs),
            board.rng,
            board.timg0.timer0,
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

        #[cfg(feature = "qa")]
        crate::qa::send_qa_resp(crate::qa::QaSignal::WifiSetup);

        let conn_settings: ConnSettings = wifi_res
            .data
            .take()
            .and_then(|d| serde_json::from_value(d).ok())
            .unwrap_or_default();

        let mut parse_retry_count = 0;
        let ws_url = loop {
            let url =
                if conn_settings.mdns || conn_settings.ws_url.is_none() || parse_retry_count > 0 {
                    log::info!("Starting mdns lookup...");
                    global_state.state.lock().await.scene = Scene::MdnsWait;
                    let mdns_res = mdns::mdns_query(wifi_res.sta_stack).await;
                    log::info!("Mdns result: {mdns_res:?}");

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
        spawner.must_spawn(ws::ws_task(
            wifi_res.sta_stack,
            ws_url,
            global_state.clone(),
            ws_sleep_sig.clone(),
        ));
        spawner.must_spawn(logger_task(global_state.clone()));

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

#[cfg(feature = "esp_now")]
#[embassy_executor::task]
async fn bc_test_task(mut tx: EspNowSender<'static>, manager: EspNowManager<'static>) {
    let mut counter = 0u128;
    loop {
        manager
            .add_peer(esp_wifi::esp_now::PeerInfo {
                peer_address: *ESP_NOW_DST,
                lmk: None,
                channel: None,
                encrypt: false,
            })
            .unwrap();

        let r = tx.send_async(ESP_NOW_DST, &counter.to_be_bytes()).await;
        manager.remove_peer(ESP_NOW_DST).unwrap();

        counter += 1;
        Timer::after_millis(1000).await;
    }
}
