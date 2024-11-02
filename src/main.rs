#![no_std]
#![no_main]

extern crate alloc;
use alloc::rc::Rc;
use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::Timer;
use embedded_hal::digital::OutputPin;
use esp_backtrace as _;
use esp_hal::{
    gpio::{Input, Io, Output},
    prelude::*,
    timer::timg::TimerGroup,
};
use esp_wifi::EspWifiInitFor;
use scenes::{GlobalState, Scene};
use structs::ConnSettings;

mod battery;
mod buttons;
mod lcd;
mod mdns;
mod rfid;
mod scenes;
mod stackmat;
mod structs;
mod ws;

/*
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}
*/

pub fn random() -> u32 {
    unsafe { &*esp_hal::peripherals::RNG::PTR }
        .data()
        .read()
        .bits()
}

#[main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config.cpu_clock = CpuClock::max();
        config
    });

    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(110 * 1024);
    let nvs = esp_hal_wifimanager::Nvs::new(0x9000, 0x6000);

    let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let sck = io.pins.gpio4.degrade();
    let miso = io.pins.gpio5.degrade();
    let mosi = io.pins.gpio6.degrade();
    let battery_input = io.pins.gpio2;
    let stackmat_rx = io.pins.gpio20.degrade();
    let button_input = Input::new(io.pins.gpio3, esp_hal::gpio::Pull::Down);
    let shifter_data_pin = Output::new(io.pins.gpio10, esp_hal::gpio::Level::Low);
    let shifter_clk_pin = Output::new(io.pins.gpio21, esp_hal::gpio::Level::Low);
    let shifter_latch_pin = Output::new(io.pins.gpio1, esp_hal::gpio::Level::Low);
    let mut adv_shift_reg = adv_shift_registers::AdvancedShiftRegister::<8, _>::new(
        shifter_data_pin,
        shifter_clk_pin,
        shifter_latch_pin,
        0,
    );

    // display digits
    let digits_shifters = adv_shift_reg.get_shifter_range_mut(2..8);
    digits_shifters.set_data(&[255; 6]);

    let buttons_shifter = adv_shift_reg.get_shifter_mut(0);
    let lcd_shifter = adv_shift_reg.get_shifter_mut(1);
    let mut cs_pin = adv_shift_reg.get_pin_mut(1, 0, true);
    _ = cs_pin.set_high();

    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);
    let global_state = Rc::new(GlobalState::new());

    let test_time_signal = alloc::rc::Rc::new(Signal::<NoopRawMutex, Option<u64>>::new());
    let lcd_change_sig = alloc::rc::Rc::new(Signal::<NoopRawMutex, u8>::new());
    _ = spawner.spawn(lcd::lcd_task(lcd_shifter, test_time_signal.clone(), lcd_change_sig.clone()));
    _ = spawner.spawn(battery::batter_read_task(battery_input, peripherals.ADC1));
    _ = spawner.spawn(buttons::buttons_task(button_input, buttons_shifter));
    _ = spawner.spawn(stackmat::stackmat_task(peripherals.UART0, stackmat_rx, test_time_signal));
    _ = spawner.spawn(rfid::rfid_task(
        miso,
        mosi,
        sck,
        cs_pin,
        peripherals.SPI2,
        peripherals.DMA,
    ));

    let mut wm_settings = esp_hal_wifimanager::WmSettings::default();
    wm_settings.ssid_generator = |efuse| {
        let mut generated_name = heapless::String::<32>::new();
        _ = core::fmt::write(&mut generated_name, format_args!("FKM-{:X}", efuse));
        generated_name
    };

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let wifi_res = esp_hal_wifimanager::init_wm(
        EspWifiInitFor::Wifi,
        wm_settings,
        timg0.timer0,
        &spawner,
        &nvs,
        rng.clone(),
        peripherals.RADIO_CLK,
        peripherals.WIFI,
        peripherals.BT,
    )
    .await
    .unwrap();

    if let Some(ref data) = wifi_res.data {
        let conn_settings: ConnSettings = serde_json::from_value(data.clone()).unwrap();
        log::info!("conn_settings: {conn_settings:?}");
    }
    log::info!("wifi_res: {:?}", wifi_res);
    /*
    scenes::CURRENT_STATE.lock().await.scene = Scene::MdnsWait;
    scenes::STATE_CHANGED.signal(());
    */

    log::info!("Start mdns lookup...");
    let mdns_option = mdns::mdns_query(&wifi_res.sta_stack).await;
    log::info!("mdns: {:?}", mdns_option);

    if let Some(ws_url) = mdns_option {
        /*
        scenes::CURRENT_STATE.lock().await.server_connected = Some(false);
        scenes::STATE_CHANGED.signal(());
        */

        _ = spawner.spawn(ws::ws_task(wifi_res.sta_stack, ws_url));
    }

    /*
    scenes::CURRENT_STATE.lock().await.scene = Scene::WaitingForCompetitor { time: None };
    scenes::STATE_CHANGED.signal(());
    */
    loop {
        log::info!("bump {}", esp_hal::time::now());
        Timer::after_millis(15000).await;
    }
}
