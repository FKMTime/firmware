#![no_std]
#![no_main]

use core::mem::MaybeUninit;
use embassy_executor::Spawner;
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::{
    clock::{ClockControl, Clocks},
    peripherals::Peripherals,
    prelude::*,
    system::SystemControl,
    timer::timg::TimerGroup,
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

    loop {
        log::info!("bump {}", esp_hal::time::current_time());
        Timer::after_millis(15000).await;
    }
}
