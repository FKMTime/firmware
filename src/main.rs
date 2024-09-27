#![no_std]
#![no_main]

use adv_shift_registers::wrappers::ShifterPin;
use core::mem::MaybeUninit;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_hal::digital::OutputPin;
use esp_backtrace as _;
use esp_hal::{
    clock::{ClockControl, Clocks},
    dma::{Dma, DmaRxBuf, DmaTxBuf},
    dma_buffers,
    gpio::{AnyOutput, AnyPin, Io},
    peripherals::{Peripherals, DMA},
    prelude::*,
    spi::{
        master::{Spi, SpiDmaBus},
        FullDuplexMode, SpiMode,
    },
    system::SystemControl,
    timer::timg::TimerGroup,
    Async,
};

extern crate alloc;

#[global_allocator]
static ALLOCATOR: esp_alloc::EspHeap = esp_alloc::EspHeap::empty();

fn init_heap() {
    const HEAP_SIZE: usize = 20 * 1024;
    static mut HEAP: MaybeUninit<[u8; HEAP_SIZE]> = MaybeUninit::uninit();

    unsafe {
        ALLOCATOR.init(HEAP.as_mut_ptr() as *mut u8, HEAP_SIZE);
    }
}

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

#[main]
async fn main(spawner: Spawner) {
    let peripherals = Peripherals::take();
    let system = SystemControl::new(peripherals.SYSTEM);
    let clocks =
        ClockControl::configure(system.clock_control, esp_hal::clock::CpuClock::Clock80MHz)
            .freeze();

    let clocks = &*mk_static!(Clocks<'static>, clocks);
    init_heap();

    esp_println::logger::init_logger_from_env();
    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let sck = AnyPin::new(io.pins.gpio4);
    let miso = AnyPin::new(io.pins.gpio5);
    let mosi = AnyPin::new(io.pins.gpio6);

    let data_pin = AnyOutput::new(io.pins.gpio10, esp_hal::gpio::Level::Low);
    let clk_pin = AnyOutput::new(io.pins.gpio21, esp_hal::gpio::Level::Low);
    let latch_pin = AnyOutput::new(io.pins.gpio1, esp_hal::gpio::Level::Low);
    let mut adv_shift_reg =
        adv_shift_registers::AdvancedShiftRegister::<8, _>::new(data_pin, clk_pin, latch_pin, 0);

    // off digits
    let digits_shifters = adv_shift_reg.get_shifter_range_mut(2..8);
    digits_shifters.set_data(&[255; 6]);

    let mut cs_pin = adv_shift_reg.get_pin_mut(1, 0, true);
    _ = cs_pin.set_high();

    let timg1 = TimerGroup::new(peripherals.TIMG1, &clocks);
    esp_hal_embassy::init(&clocks, timg1.timer0);

    let rng = esp_hal::rng::Rng::new(peripherals.RNG);

    let mut wm_settings = esp_hal_wifimanager::WmSettings::default();
    wm_settings.ssid_generator = |efuse| {
        let mut generated_name = heapless::String::<32>::new();
        _ = core::fmt::write(&mut generated_name, format_args!("TEST-{:X}", efuse));
        generated_name
    };

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0, &clocks);
    let wifi_res = esp_hal_wifimanager::init_wm(
        wm_settings,
        timg0.timer0,
        rng.clone(),
        peripherals.RADIO_CLK,
        &clocks,
        peripherals.WIFI,
        peripherals.BT,
        &spawner,
    )
    .await;

    log::info!("wifi_res: {wifi_res:?}");

    _ = spawner.spawn(rfid_task(
        miso,
        mosi,
        sck,
        cs_pin,
        &clocks,
        peripherals.SPI2,
        peripherals.DMA,
    ));

    loop {
        log::info!("bump {}", esp_hal::time::current_time());
        Timer::after_millis(15000).await;
    }
}

#[embassy_executor::task]
async fn rfid_task(
    miso: AnyPin<'static>,
    mosi: AnyPin<'static>,
    sck: AnyPin<'static>,
    cs_pin: ShifterPin,
    clocks: &'static Clocks<'static>,
    spi: esp_hal::peripherals::SPI2,
    dma: DMA,
) {
    let dma = Dma::new(dma);
    let dma_chan = dma.channel0;
    let (tx_buffer, tx_descriptors, rx_buffer, rx_descriptors) = dma_buffers!(32000);
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();

    let dma_chan = dma_chan.configure_for_async(false, esp_hal::dma::DmaPriority::Priority0);

    //let cs = Output::new(cs, Level::High);
    let spi = Spi::new(spi, 5.MHz(), SpiMode::Mode0, &clocks);
    let spi: Spi<_, FullDuplexMode> = spi.with_sck(sck).with_miso(miso).with_mosi(mosi);
    let spi: SpiDmaBus<_, _, FullDuplexMode, Async> =
        spi.with_dma(dma_chan).with_buffers(dma_tx_buf, dma_rx_buf);

    let mut mfrc522 = esp_hal_mfrc522::MFRC522::new(spi, cs_pin);
    _ = mfrc522.pcd_init().await;
    log::debug!("PCD ver: {:?}", mfrc522.pcd_get_version().await);

    if !mfrc522.pcd_is_init().await {
        log::error!("MFRC522 init failed! Try to power cycle to module!");
    }

    loop {
        if mfrc522.picc_is_new_card_present().await.is_ok() {
            let card = mfrc522
                .get_card(esp_hal_mfrc522::consts::UidSize::Four)
                .await;
            if let Ok(card) = card {
                log::info!("Card UID: {}", card.get_number());
            }

            _ = mfrc522.picc_halta().await;
        }

        Timer::after(Duration::from_millis(10)).await;
    }
}
