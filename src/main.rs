#![no_std]
#![no_main]

extern crate alloc;
use adv_shift_registers::wrappers::ShifterPin;
use battery::batter_read_task;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_hal::digital::OutputPin;
use esp_backtrace as _;
use esp_hal::{
    dma::{Dma, DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::{AnyPin, Io, Output},
    peripherals::{self, DMA, UART0},
    prelude::*,
    spi::{master::Spi, SpiMode},
    timer::timg::TimerGroup,
    uart::UartRx,
};
use esp_hal_mfrc522::consts::UidSize;
use esp_wifi::EspWifiInitFor;
use structs::ConnSettings;

mod battery;
mod mdns;
mod structs;

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

    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let sck = io.pins.gpio4.degrade();
    let miso = io.pins.gpio5.degrade();
    let mosi = io.pins.gpio6.degrade();
    let battery_input_pin = io.pins.gpio2;
    let stackmat_rx = io.pins.gpio20.degrade();

    _ = spawner.spawn(batter_read_task(battery_input_pin, peripherals.ADC1));

    let data_pin = Output::new(io.pins.gpio10, esp_hal::gpio::Level::Low);
    let clk_pin = Output::new(io.pins.gpio21, esp_hal::gpio::Level::Low);
    let latch_pin = Output::new(io.pins.gpio1, esp_hal::gpio::Level::Low);
    let mut adv_shift_reg =
        adv_shift_registers::AdvancedShiftRegister::<8, _>::new(data_pin, clk_pin, latch_pin, 0);

    // off digits
    let digits_shifters = adv_shift_reg.get_shifter_range_mut(2..8);
    digits_shifters.set_data(&[255; 6]);

    let mut cs_pin = adv_shift_reg.get_pin_mut(1, 0, true);
    _ = cs_pin.set_high();

    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);

    let rng = esp_hal::rng::Rng::new(peripherals.RNG);

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
        rng.clone(),
        peripherals.RADIO_CLK,
        peripherals.WIFI,
        peripherals.BT,
        &spawner,
        &nvs,
    )
    .await
    .unwrap();

    if let Some(ref data) = wifi_res.data {
        let conn_settings: ConnSettings = serde_json::from_value(data.clone()).unwrap();
        log::info!("conn_settings: {conn_settings:?}");
    }
    log::info!("wifi_res: {:?}", wifi_res);

    _ = spawner.spawn(stackmat_task(peripherals.UART0, stackmat_rx));
    _ = spawner.spawn(rfid_task(
        miso,
        mosi,
        sck,
        cs_pin,
        peripherals.SPI2,
        peripherals.DMA,
    ));

    log::info!("Start mdns lookup...");
    let mdns_option = mdns::mdns_query(&wifi_res.sta_stack).await;
    log::info!("mdns: {:?}", mdns_option);

    loop {
        log::info!("bump {}", esp_hal::time::now());
        Timer::after_millis(15000).await;
    }
}

#[embassy_executor::task]
async fn stackmat_task(uart: UART0, uart_pin: AnyPin) {
    let mut uart = UartRx::new_async(uart, uart_pin).unwrap();
    let mut buf = [0; 30];
    loop {
        if let Ok(n) = embedded_io_async::Read::read(&mut uart, &mut buf).await {
            log::warn!("uart read byte (n:{n}): {:?}", &buf);
        }
    }
}

#[embassy_executor::task]
async fn rfid_task(
    miso: AnyPin,
    mosi: AnyPin,
    sck: AnyPin,
    cs_pin: ShifterPin,
    spi: esp_hal::peripherals::SPI2,
    dma: DMA,
) {
    let dma = Dma::new(dma);
    let dma_chan = dma.channel0;
    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(32000);
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();

    let dma_chan = dma_chan.configure_for_async(false, esp_hal::dma::DmaPriority::Priority0);

    //let cs = Output::new(cs, Level::High);
    let spi = Spi::new(spi, 5.MHz(), SpiMode::Mode0);
    let spi = spi.with_sck(sck).with_miso(miso).with_mosi(mosi);
    let spi = spi.with_dma(dma_chan).with_buffers(dma_rx_buf, dma_tx_buf);

    //esp_hal_mfrc522::MFRC522::new(spi, cs, || esp_hal::time::current_time().ticks());
    let mut mfrc522 = esp_hal_mfrc522::MFRC522::new(spi, cs_pin); // embassy-time feature is enabled,
                                                                  // so no need to pass current_time
                                                                  // function

    _ = mfrc522.pcd_init().await;
    _ = mfrc522.pcd_selftest().await;
    log::debug!("PCD ver: {:?}", mfrc522.pcd_get_version().await);

    if !mfrc522.pcd_is_init().await {
        log::error!("MFRC522 init failed! Try to power cycle to module!");
    }

    loop {
        if mfrc522.picc_is_new_card_present().await.is_ok() {
            let card = mfrc522.get_card(UidSize::Four).await;
            if let Ok(card) = card {
                log::info!("Card UID: {}", card.get_number());

                let mut buff = [0; 18];
                let mut byte_count = 18;
                _ = mfrc522.mifare_read(0, &mut buff, &mut byte_count).await;

                log::info!("{:02X?}", buff);

                //_ = mfrc522.debug_dump_card(&card).await;
            }

            _ = mfrc522.picc_halta().await;
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}
