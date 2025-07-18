use crate::utils::stackmat::{DEC_DIGITS, DOT_MOD};
use adv_shift_registers::wrappers::ShifterValueRange;
use anyhow::Result;
use esp_hal::{
    gpio::{AnyPin, Input, InputConfig, Level, Output, Pin, Pull},
    peripherals::{ADC1, BT, Peripherals, SPI2, TIMG0, TIMG1, UART1, WIFI},
    rng::Rng,
    timer::timg::TimerGroup,
};

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

    // spi
    pub miso: AnyPin<'static>,
    pub mosi: AnyPin<'static>,
    pub sck: AnyPin<'static>,
    pub cs: adv_shift_registers::wrappers::ShifterPin,

    pub stackmat_rx: AnyPin<'static>,

    pub battery: esp_hal::peripherals::GPIO2<'static>,
    pub button_input: Input<'static>,
    pub digits_shifters: ShifterValueRange,

    pub buttons_shifter: adv_shift_registers::wrappers::ShifterValue,
    pub lcd: adv_shift_registers::wrappers::ShifterValue,
}

impl Board {
    pub fn init(peripherals: Peripherals) -> Result<Board> {
        use embedded_hal::digital::OutputPin;

        let timg0 = TimerGroup::new(peripherals.TIMG0);
        let timg1 = TimerGroup::new(peripherals.TIMG1);
        let rng = Rng::new(peripherals.RNG);
        let uart1 = peripherals.UART1;
        let spi2 = peripherals.SPI2;
        let spi_dma = peripherals.DMA_CH0;
        let adc1 = peripherals.ADC1;
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
