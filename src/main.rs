#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![feature(asm_experimental_arch)]

extern crate alloc;
use alloc::{rc::Rc, vec::Vec};
use consts::{LOG_SEND_INTERVAL_MS, PRINT_HEAP_INTERVAL_MS};
use core::str::FromStr;
use embassy_executor::Spawner;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use esp_backtrace as _;
use esp_hal::gpio::Pin;
use esp_hal::{
    gpio::{Input, Output},
    timer::timg::TimerGroup,
};
use esp_storage::FlashStorage;
use state::{get_ota_state, GlobalStateInner, SavedGlobalState, Scene};
use structs::ConnSettings;
use translations::init_translations;
use utils::{logger::FkmLogger, set_brownout_detection};

#[cfg(feature = "esp32")]
use esp_hal::time::RateExtU32;

mod battery;
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
mod ws;

#[cfg_attr(not(feature = "gen_version"), path = "version_fallback.rs")]
#[cfg_attr(feature = "gen_version", path = "version.rs")]
mod version;

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
        config.cpu_clock = esp_hal::clock::CpuClock::max();
        config
    });

    FkmLogger::set_logger();
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);

    #[cfg(not(feature = "esp32"))]
    {
        esp_alloc::heap_allocator!(120 * 1024);
    }

    // NOTE: on esp32 (generic) use only dram2 region as heap
    #[cfg(feature = "esp32")]
    {
        #[link_section = ".dram2_uninit"]
        static mut HEAP2: core::mem::MaybeUninit<[u8; 90 * 1024]> =
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

    let nvs = esp_hal_wifimanager::Nvs::new_from_part_table().unwrap();

    set_brownout_detection(false);
    let rng = esp_hal::rng::Rng::new(peripherals.RNG);

    #[cfg(feature = "esp32")]
    let sck = peripherals.GPIO18.degrade();
    #[cfg(feature = "esp32")]
    let miso = peripherals.GPIO19.degrade();
    #[cfg(feature = "esp32")]
    let mosi = peripherals.GPIO23.degrade();
    #[cfg(feature = "esp32")]
    let battery_input = peripherals.GPIO34;
    #[cfg(feature = "esp32")]
    let stackmat_rx = peripherals.GPIO4.degrade();
    #[cfg(feature = "esp32")]
    let shifter_data_pin = Output::new(peripherals.GPIO16, esp_hal::gpio::Level::Low);
    #[cfg(feature = "esp32")]
    let shifter_clk_pin = Output::new(peripherals.GPIO12, esp_hal::gpio::Level::Low);
    #[cfg(feature = "esp32")]
    let shifter_latch_pin = Output::new(peripherals.GPIO17, esp_hal::gpio::Level::Low);

    #[cfg(feature = "esp32c3")]
    let sck = peripherals.GPIO4.degrade();
    #[cfg(feature = "esp32c3")]
    let miso = peripherals.GPIO5.degrade();
    #[cfg(feature = "esp32c3")]
    let mosi = peripherals.GPIO6.degrade();
    #[cfg(feature = "esp32c3")]
    let battery_input = peripherals.GPIO2;
    #[cfg(feature = "esp32c3")]
    let stackmat_rx = peripherals.GPIO20.degrade();
    #[cfg(feature = "esp32c3")]
    let shifter_data_pin = Output::new(peripherals.GPIO10, esp_hal::gpio::Level::Low);
    #[cfg(feature = "esp32c3")]
    let shifter_latch_pin = Output::new(peripherals.GPIO1, esp_hal::gpio::Level::Low);
    #[cfg(feature = "esp32c3")]
    let shifter_clk_pin = if crate::utils::get_efuse_u32() == 1342310409 {
        // TODO: remove this if
        Output::new(peripherals.GPIO7, esp_hal::gpio::Level::Low)
    } else {
        Output::new(peripherals.GPIO21, esp_hal::gpio::Level::Low)
    };

    let mut adv_shift_reg = adv_shift_registers::AdvancedShiftRegister::<8, _>::new(
        shifter_data_pin,
        shifter_clk_pin,
        shifter_latch_pin,
        0,
    );

    // display digits
    #[cfg(feature = "esp32c3")]
    let digits_shifters = adv_shift_reg.get_shifter_range_mut(2..8);

    #[cfg(feature = "esp32")]
    let digits_shifters = adv_shift_reg.get_shifter_range_mut(0..6);

    digits_shifters
        .set_data(&[!crate::utils::stackmat::DEC_DIGITS[8] ^ crate::utils::stackmat::DOT_MOD; 6]);

    #[cfg(feature = "esp32")]
    let button_1 = Input::new(peripherals.GPIO27, esp_hal::gpio::Pull::Up);
    #[cfg(feature = "esp32")]
    let button_2 = Input::new(peripherals.GPIO26, esp_hal::gpio::Pull::Up);
    #[cfg(feature = "esp32")]
    let button_3 = Input::new(peripherals.GPIO33, esp_hal::gpio::Pull::Up);
    #[cfg(feature = "esp32")]
    let button_4 = Input::new(peripherals.GPIO32, esp_hal::gpio::Pull::Up);

    #[cfg(feature = "esp32c3")]
    let button_input = Input::new(peripherals.GPIO3, esp_hal::gpio::Pull::Down);

    #[cfg(feature = "esp32c3")]
    let buttons_shifter = adv_shift_reg.get_shifter_mut(0);

    #[cfg(feature = "esp32c3")]
    let lcd_shifter = adv_shift_reg.get_shifter_mut(1);

    #[cfg(feature = "esp32c3")]
    let cs_pin = {
        use embedded_hal::digital::OutputPin;

        let mut cs_pin = adv_shift_reg.get_pin_mut(1, 0, true);
        _ = cs_pin.set_high();
        cs_pin
    };

    #[cfg(feature = "esp32")]
    let cs_pin = Output::new(peripherals.GPIO5, esp_hal::gpio::Level::High);

    init_translations();
    let global_state = Rc::new(GlobalStateInner::new(&nvs));
    let wifi_setup_sig = Rc::new(Signal::new());

    #[cfg(feature = "esp32")]
    let i2c = esp_hal::i2c::master::I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default()
            .with_frequency(100.kHz())
            .with_timeout(esp_hal::i2c::master::BusTimeout::Maximum),
    )
    .unwrap()
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22);

    _ = spawner.spawn(lcd::lcd_task(
        #[cfg(feature = "esp32c3")]
        lcd_shifter,
        #[cfg(feature = "esp32")]
        i2c,
        global_state.clone(),
        wifi_setup_sig.clone(),
    ));

    _ = spawner.spawn(battery::battery_read_task(
        battery_input,
        peripherals.ADC1,
        global_state.clone(),
    ));
    _ = spawner.spawn(buttons::buttons_task(
        global_state.clone(),
        #[cfg(feature = "esp32")]
        [button_1, button_2, button_3, button_4],
        #[cfg(feature = "esp32c3")]
        button_input,
        #[cfg(feature = "esp32c3")]
        buttons_shifter,
    ));
    _ = spawner.spawn(stackmat::stackmat_task(
        peripherals.UART1,
        stackmat_rx,
        digits_shifters,
        global_state.clone(),
    ));
    _ = spawner.spawn(rfid::rfid_task(
        miso,
        mosi,
        sck,
        cs_pin,
        peripherals.SPI2,
        #[cfg(feature = "esp32c3")]
        peripherals.DMA_CH0,
        #[cfg(feature = "esp32")]
        peripherals.DMA_SPI2,
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

    {
        let mut ota = esp_hal_ota::Ota::new(FlashStorage::new()).unwrap();
        ota.ota_mark_app_valid().unwrap();
    }

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let mut wifi_res = esp_hal_wifimanager::init_wm(
        wm_settings,
        &spawner,
        &nvs,
        rng,
        timg0.timer0,
        peripherals.RADIO_CLK,
        peripherals.WIFI,
        peripherals.BT,
        Some(wifi_setup_sig),
    )
    .await
    .unwrap();

    let conn_settings: Option<ConnSettings> = wifi_res
        .data
        .take()
        .and_then(|d| serde_json::from_value(d).ok());

    let ws_url = if conn_settings.is_none()
        || conn_settings.as_ref().unwrap().mdns
        || conn_settings.as_ref().unwrap().ws_url.is_none()
    {
        log::info!("Start mdns lookup...");
        global_state.state.lock().await.scene = Scene::MdnsWait;
        let mdns_res = mdns::mdns_query(wifi_res.sta_stack).await;
        log::info!("Mdns result: {:?}", mdns_res);

        alloc::string::String::from_str(&mdns_res.expect("MDNS HOW?")).unwrap()
    } else {
        conn_settings.unwrap().ws_url.unwrap()
    };

    _ = spawner.spawn(ws::ws_task(
        wifi_res.sta_stack,
        ws_url,
        global_state.clone(),
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

    utils::backtrace_store::read_saved_backtrace().await;

    let mut heap_start = Instant::now();
    loop {
        Timer::after_millis(LOG_SEND_INTERVAL_MS).await;

        // TODO: move to own task
        let mut tmp_logs: Vec<structs::LogData> = Vec::new();
        while let Ok(msg) = utils::logger::LOGS_CHANNEL.try_receive() {
            tmp_logs.push(structs::LogData { millis: 0, msg });
        }

        if get_ota_state() {
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
            log::info!("Heap info:");
            log::info!("Size: {}", esp_alloc::HEAP.used() + esp_alloc::HEAP.free());
            log::info!("Used: {}", esp_alloc::HEAP.used());
            log::info!("Free: {}", esp_alloc::HEAP.free());

            heap_start = Instant::now();
        }
    }
}
