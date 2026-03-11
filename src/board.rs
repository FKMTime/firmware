use esp_hal::{
    gpio::{AnyPin, Input, InputConfig, Output, Pin, Pull},
    i2c::master::I2c,
    peripherals::{
        ADC1, AES, BT, FLASH, Peripherals, SPI2, SW_INTERRUPT, TIMG0, TIMG1, UART1, WIFI,
    },
    rng::Rng,
    time::Rate,
    timer::timg::TimerGroup,
};

#[cfg(feature = "v3")]
use crate::utils::stackmat::{DEC_DIGITS, DOT_MOD};

#[cfg(feature = "v4")]
use crate::utils::shared_i2c::SharedI2C;

#[allow(dead_code)]
pub struct Board {
    // peripherals
    pub timg0: TimerGroup<'static, TIMG0<'static>>,
    pub timg1: TimerGroup<'static, TIMG1<'static>>,
    pub rng: Rng,
    pub uart1: UART1<'static>,
    pub spi2: SPI2<'static>,
    pub adc1: ADC1<'static>,
    pub wifi: WIFI<'static>,
    pub bt: BT<'static>,
    pub spi_dma: esp_hal::peripherals::DMA_CH0<'static>,
    pub aes: AES<'static>,
    pub flash: FLASH<'static>,
    pub sw_interrupt: SW_INTERRUPT<'static>,

    // spi
    #[cfg(feature = "v3")]
    pub miso: AnyPin<'static>,
    #[cfg(feature = "v3")]
    pub mosi: AnyPin<'static>,
    #[cfg(feature = "v3")]
    pub sck: AnyPin<'static>,
    #[cfg(feature = "v3")]
    pub cs: adv_shift_registers::wrappers::ShifterPin,

    #[cfg(feature = "v4")]
    pub display_rst: Output<'static>,
    #[cfg(feature = "v4")]
    pub i2c: SharedI2C,

    pub stackmat_rx: AnyPin<'static>,

    #[cfg(feature = "v4")]
    pub buttons: [Input<'static>; 4],

    #[cfg(feature = "v3")]
    pub battery: esp_hal::peripherals::GPIO2<'static>,
    #[cfg(feature = "v3")]
    pub button_input: Input<'static>,
    #[cfg(feature = "v3")]
    pub digits_shifters: adv_shift_registers::wrappers::ShifterValueRange,

    #[cfg(feature = "v3")]
    pub buttons_shifter: adv_shift_registers::wrappers::ShifterValue,
    #[cfg(feature = "v3")]
    pub lcd: adv_shift_registers::wrappers::ShifterValue,

    // usb pins
    pub usb_dp: AnyPin<'static>,
    pub usb_dm: AnyPin<'static>,
}

impl Board {
    pub fn init(peripherals: Peripherals) -> Board {
        let timg0 = TimerGroup::new(peripherals.TIMG0);
        let timg1 = TimerGroup::new(peripherals.TIMG1);
        let rng = Rng::new();
        let uart1 = peripherals.UART1;
        let spi2 = peripherals.SPI2;
        let spi_dma = peripherals.DMA_CH0;
        let adc1 = peripherals.ADC1;
        let wifi = peripherals.WIFI;
        let bt = peripherals.BT;
        let aes = peripherals.AES;
        let flash = peripherals.FLASH;
        let sw_interrupt = peripherals.SW_INTERRUPT;

        let stackmat_rx = peripherals.GPIO20.degrade();
        let usb_dp = peripherals.GPIO19.degrade();
        let usb_dm = peripherals.GPIO18.degrade();

        let display_rst = Output::new(
            peripherals.GPIO7,
            esp_hal::gpio::Level::Low,
            Default::default(),
        );

        let b1 = Input::new(
            peripherals.GPIO0,
            InputConfig::default().with_pull(Pull::Down),
        );

        let b2 = Input::new(
            peripherals.GPIO1,
            InputConfig::default().with_pull(Pull::Down),
        );

        let b3 = Input::new(
            peripherals.GPIO2,
            InputConfig::default().with_pull(Pull::Down),
        );

        let b4 = Input::new(
            peripherals.GPIO3,
            InputConfig::default().with_pull(Pull::Down),
        );

        let Ok(i2c) = I2c::new(
            peripherals.I2C0,
            esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
        ) else {
            log::error!("Rfid task error while creating Spi instance!");
            panic!()
        };
        let i2c = i2c
            .with_sda(peripherals.GPIO8)
            .with_scl(peripherals.GPIO9)
            .into_async();
        let i2c = SharedI2C::new(i2c);

        Board {
            timg0,
            timg1,
            rng,
            uart1,
            spi2,
            spi_dma,
            adc1,
            wifi,
            bt,
            aes,
            flash,
            sw_interrupt,

            i2c,

            display_rst,
            stackmat_rx,
            buttons: [b1, b2, b3, b4],

            usb_dp,
            usb_dm,
        }
    }
}
