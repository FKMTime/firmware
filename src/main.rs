#![no_std]
#![no_main]

extern crate alloc;
use alloc::rc::Rc;
use core::str::FromStr;
use embassy_executor::Spawner;
use embassy_time::Timer;
use embedded_hal::digital::OutputPin;
use esp_backtrace as _;
use esp_hal::{
    gpio::{Input, Output},
    prelude::*,
    timer::timg::TimerGroup,
};
use state::{GlobalStateInner, Scene};
use structs::ConnSettings;
use utils::set_brownout_detection;

mod battery;
mod buttons;
mod lcd;
mod mdns;
mod rfid;
mod stackmat;
mod state;
mod structs;
mod utils;
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

#[main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config.cpu_clock = CpuClock::max();
        config
    });

    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(120 * 1024);
    let nvs = esp_hal_wifimanager::Nvs::new(0x9000, 0x4000);

    set_brownout_detection(false);
    let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    let sck = peripherals.GPIO4.degrade();
    let miso = peripherals.GPIO5.degrade();
    let mosi = peripherals.GPIO6.degrade();
    let battery_input = peripherals.GPIO2;
    let stackmat_rx = peripherals.GPIO20.degrade();
    let button_input = Input::new(peripherals.GPIO3, esp_hal::gpio::Pull::Down);
    let shifter_data_pin = Output::new(peripherals.GPIO10, esp_hal::gpio::Level::Low);
    let shifter_clk_pin = Output::new(peripherals.GPIO21, esp_hal::gpio::Level::Low);
    let shifter_latch_pin = Output::new(peripherals.GPIO1, esp_hal::gpio::Level::Low);
    let mut adv_shift_reg = adv_shift_registers::AdvancedShiftRegister::<8, _>::new(
        shifter_data_pin,
        shifter_clk_pin,
        shifter_latch_pin,
        0,
    );

    // display digits
    let digits_shifters = adv_shift_reg.get_shifter_range_mut(2..8);
    digits_shifters
        .set_data(&[!crate::utils::stackmat::DEC_DIGITS[8] ^ crate::utils::stackmat::DOT_MOD; 6]);

    let buttons_shifter = adv_shift_reg.get_shifter_mut(0);
    let lcd_shifter = adv_shift_reg.get_shifter_mut(1);
    let mut cs_pin = adv_shift_reg.get_pin_mut(1, 0, true);
    _ = cs_pin.set_high();

    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);
    let global_state = Rc::new(GlobalStateInner::new(&nvs));

    _ = spawner.spawn(lcd::lcd_task(lcd_shifter, global_state.clone()));
    _ = spawner.spawn(battery::batter_read_task(battery_input, peripherals.ADC1));
    _ = spawner.spawn(buttons::buttons_task(
        button_input,
        buttons_shifter,
        global_state.clone(),
    ));
    _ = spawner.spawn(stackmat::stackmat_task(
        peripherals.UART0,
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
        peripherals.DMA,
        global_state.clone(),
    ));

    let mut wm_settings = esp_hal_wifimanager::WmSettings::default();
    wm_settings.ssid_generator = |_efuse| {
        let mut generated_name = heapless::String::<32>::new();
        _ = core::fmt::write(
            &mut generated_name,
            format_args!("FKM-{:X}", esp_hal_wifimanager::get_efuse_u32()),
        );
        generated_name
    };

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let mut wifi_res = esp_hal_wifimanager::init_wm(
        wm_settings,
        &spawner,
        &nvs,
        rng.clone(),
        timg0.timer0,
        peripherals.RADIO_CLK,
        peripherals.WIFI,
        peripherals.BT,
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
        let mdns_res = mdns::mdns_query(&wifi_res.sta_stack).await;
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
    loop {
        //log::info!("bump {}", esp_hal::time::now());
        Timer::after_millis(15000).await;
    }
}
