use crate::utils::stackmat::{DEC_DIGITS, DOT_MOD};
use adv_shift_registers::wrappers::ShifterValueRange;
use anyhow::Result;
use esp_hal::{
    gpio::{AnyPin, GpioPin, Input, InputConfig, Level, Output, Pin, Pull},
    peripherals::{Peripherals, ADC1, BT, RADIO_CLK, SPI2, TIMG0, TIMG1, UART1, WIFI},
    rng::Rng,
    timer::timg::TimerGroup,
};

pub struct Board {
    // peripherals
    pub timg0: TimerGroup<TIMG0>,
    pub timg1: TimerGroup<TIMG1>,
    pub rng: Rng,
    pub uart1: UART1,
    pub spi2: SPI2,
    pub adc1: ADC1,
    pub radio_clk: RADIO_CLK,
    pub wifi: WIFI,
    pub bt: BT,
    #[cfg(feature = "esp32c3")]
    pub spi_dma: esp_hal::dma::DmaChannel0,
    #[cfg(feature = "esp32")]
    pub spi_dma: esp_hal::dma::Spi2DmaChannel,

    // spi
    pub miso: AnyPin,
    pub mosi: AnyPin,
    pub sck: AnyPin,
    #[cfg(feature = "esp32c3")]
    pub cs: adv_shift_registers::wrappers::ShifterPin,
    #[cfg(feature = "esp32")]
    pub cs: Output<'static>,

    pub stackmat_rx: AnyPin,

    #[cfg(feature = "esp32c3")]
    pub battery: GpioPin<2>,
    #[cfg(feature = "esp32")]
    pub battery: GpioPin<34>,

    #[cfg(feature = "esp32c3")]
    pub button_input: Input<'static>,
    #[cfg(feature = "esp32")]
    pub button_input: [Input<'static>; 4],

    pub digits_shifters: ShifterValueRange,

    #[cfg(feature = "esp32c3")]
    pub buttons_shifter: adv_shift_registers::wrappers::ShifterValue,
    #[cfg(feature = "esp32c3")]
    pub lcd: adv_shift_registers::wrappers::ShifterValue,
    #[cfg(feature = "esp32")]
    pub lcd: esp_hal::i2c::master::I2c<'static, esp_hal::Blocking>,
}

#[cfg(feature = "esp32c3")]
impl Board {
    pub fn init(peripherals: Peripherals) -> Result<Board> {
        use embedded_hal::digital::OutputPin;

        esp_alloc::heap_allocator!(size: 120 * 1024);

        let timg0 = TimerGroup::new(peripherals.TIMG0);
        let timg1 = TimerGroup::new(peripherals.TIMG1);
        let rng = Rng::new(peripherals.RNG);
        let uart1 = peripherals.UART1;
        let spi2 = peripherals.SPI2;
        let spi_dma = peripherals.DMA_CH0;
        let adc1 = peripherals.ADC1;
        let radio_clk = peripherals.RADIO_CLK;
        let wifi = peripherals.WIFI;
        let bt = peripherals.BT;

        let sck = peripherals.GPIO4.degrade();
        let miso = peripherals.GPIO5.degrade();
        let mosi = peripherals.GPIO6.degrade();
        let battery = peripherals.GPIO2;
        let stackmat_rx = peripherals.GPIO20.degrade();

        let button_input = Input::new(
            peripherals.GPIO3,
            InputConfig::default().with_pull(Pull::Down),
        );

        let shifter_data_pin = Output::new(peripherals.GPIO10, Level::Low, Default::default());
        let shifter_latch_pin = Output::new(peripherals.GPIO1, Level::Low, Default::default());
        let shifter_clk_pin = Output::new(peripherals.GPIO21, Level::Low, Default::default());

        let adv_shift_reg = adv_shift_registers::AdvancedShiftRegister::<8, _>::new(
            shifter_data_pin,
            shifter_clk_pin,
            shifter_latch_pin,
            0,
        );
        let adv_shift_reg = alloc::boxed::Box::new(adv_shift_reg);
        let adv_shift_reg = alloc::boxed::Box::leak(adv_shift_reg);

        let mut backlight = adv_shift_reg.get_pin_mut(1, 1, false);
        _ = backlight.set_high();

        let buttons_shifter = adv_shift_reg.get_shifter_mut(0);
        let lcd = adv_shift_reg.get_shifter_mut(1);
        let digits_shifters = adv_shift_reg.get_shifter_range_mut(2..8);
        digits_shifters.set_data(&[!DEC_DIGITS[8] ^ DOT_MOD; 6]);

        let mut cs = adv_shift_reg.get_pin_mut(1, 0, true);
        _ = cs.set_high();

        Ok(Board {
            timg0,
            timg1,
            rng,
            uart1,
            spi2,
            spi_dma,
            adc1,
            radio_clk,
            wifi,
            bt,

            miso,
            mosi,
            sck,
            cs,

            battery,
            stackmat_rx,
            button_input,

            buttons_shifter,
            digits_shifters,
            lcd,
        })
    }
}

#[cfg(feature = "esp32")]
impl Board {
    pub fn init(peripherals: Peripherals) -> Result<Board> {
        use embedded_hal::digital::OutputPin;

        let timg0 = TimerGroup::new(peripherals.TIMG0);
        let timg1 = TimerGroup::new(peripherals.TIMG1);
        let rng = Rng::new(peripherals.RNG);
        let uart1 = peripherals.UART1;
        let spi2 = peripherals.SPI2;
        let spi_dma = peripherals.DMA_SPI2;
        let adc1 = peripherals.ADC1;
        let radio_clk = peripherals.RADIO_CLK;
        let wifi = peripherals.WIFI;
        let bt = peripherals.BT;

        let sck = peripherals.GPIO18.degrade();
        let miso = peripherals.GPIO19.degrade();
        let mosi = peripherals.GPIO23.degrade();
        let cs = Output::new(peripherals.GPIO5, Level::High, Default::default());
        let battery = peripherals.GPIO34;
        let stackmat_rx = peripherals.GPIO4.degrade();

        let button1 = Input::new(
            peripherals.GPIO27,
            InputConfig::default().with_pull(Pull::Up),
        );
        let button2 = Input::new(
            peripherals.GPIO26,
            InputConfig::default().with_pull(Pull::Up),
        );
        let button3 = Input::new(
            peripherals.GPIO33,
            InputConfig::default().with_pull(Pull::Up),
        );
        let button4 = Input::new(
            peripherals.GPIO32,
            InputConfig::default().with_pull(Pull::Up),
        );

        let shifter_data_pin = Output::new(peripherals.GPIO16, Level::Low, Default::default());
        let shifter_latch_pin = Output::new(peripherals.GPIO12, Level::Low, Default::default());
        let shifter_clk_pin = Output::new(peripherals.GPIO17, Level::Low, Default::default());

        let adv_shift_reg = adv_shift_registers::AdvancedShiftRegister::<8, _>::new(
            shifter_data_pin,
            shifter_clk_pin,
            shifter_latch_pin,
            0,
        );
        let adv_shift_reg = alloc::boxed::Box::new(adv_shift_reg);
        let adv_shift_reg = alloc::boxed::Box::leak(adv_shift_reg);

        let mut backlight = adv_shift_reg.get_pin_mut(1, 1, false);
        _ = backlight.set_high();

        let digits_shifters = adv_shift_reg.get_shifter_range_mut(0..6);
        digits_shifters.set_data(&[!DEC_DIGITS[8] ^ DOT_MOD; 6]);

        let lcd = esp_hal::i2c::master::I2c::new(
            peripherals.I2C0,
            esp_hal::i2c::master::Config::default()
                .with_frequency(esp_hal::time::Rate::from_khz(100))
                .with_timeout(esp_hal::i2c::master::BusTimeout::Maximum),
        )?
        .with_sda(peripherals.GPIO21)
        .with_scl(peripherals.GPIO22);

        Ok(Board {
            timg0,
            timg1,
            rng,
            uart1,
            spi2,
            spi_dma,
            adc1,
            radio_clk,
            wifi,
            bt,

            miso,
            mosi,
            sck,
            cs,

            battery,
            stackmat_rx,
            button_input: [button1, button2, button3, button4],

            lcd,
            digits_shifters,
        })
    }
}
