use embassy_time::Timer;
use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation},
    gpio::GpioPin,
};

type AdcCal = esp_hal::analog::adc::AdcCalBasic<esp_hal::peripherals::ADC1>;
const BAT_MIN: f64 = 3300.0;
const BAT_MAX: f64 = 4200.0;
/*
const R1: f64 = 6900.0;
const R2: f64 = 10000.0;
*/

#[embassy_executor::task]
pub async fn batter_read_task(adc_pin: GpioPin<2>, adc: esp_hal::peripherals::ADC1) {
    let mut adc_config = AdcConfig::new();
    let mut adc_pin =
        adc_config.enable_pin_with_cal::<_, AdcCal>(adc_pin, Attenuation::Attenuation11dB);
    let mut adc = Adc::new(adc, adc_config);

    let mut count = 0;

    let base_freq = 2.0;
    let sample_freq = 1000.0;
    let sensitivity = 0.5;
    let mut smoother = dyn_smooth::DynamicSmootherEcoF32::new(base_freq, sample_freq, sensitivity);
    loop {
        Timer::after_millis(500).await;
        let read = macros::nb_to_fut!(adc.read_oneshot(&mut adc_pin))
            .await
            .unwrap_or(0);
        let read = smoother.tick(read as f32);

        count += 1;

        if count < 30 {
            // 15s
            continue;
        }

        count = 0;
        let bat_calc_mv = calculate(read as f64);
        let bat_percentage = bat_perctentage(bat_calc_mv);
        log::info!("calc({read}): {bat_calc_mv}mV {bat_percentage}%");
    }
}

fn bat_perctentage(mv: f64) -> u8 {
    if mv <= BAT_MIN {
        return 0;
    }

    if mv >= BAT_MAX {
        return 100;
    }

    return (((mv - BAT_MIN) / (BAT_MAX - BAT_MIN)) * 100.0) as u8;
}

// TODO: measure cooficients on real pcb
// https://www.dcode.fr/function-equation-finder
fn calculate(x: f64) -> f64 {
    let coefficient_1 = -0.000447414;
    let coefficient_2 = 4.56829;
    let constant = -4999.37;

    coefficient_1 * x * x + coefficient_2 * x + constant
}
