use crate::{consts::BATTERY_SEND_INTERVAL_MS, utils::rolling_average::RollingAverage};
use embassy_time::{Instant, Timer};
use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation},
    gpio::GpioPin,
};

#[cfg(feature = "esp32c3")]
type AdcCal = esp_hal::analog::adc::AdcCalBasic<esp_hal::peripherals::ADC1>;

const BAT_MIN: f64 = 3200.0;
const BAT_MAX: f64 = 4200.0;
const BATTERY_CURVE: [(f64, u8); 11] = [
    (3200.0, 0),
    (3250.0, 5),
    (3300.0, 10),
    (3350.0, 20),
    (3400.0, 30),
    (3500.0, 40),
    (3600.0, 50),
    (3700.0, 60),
    (3800.0, 70),
    (3900.0, 80),
    (4200.0, 100),
];

#[embassy_executor::task]
pub async fn battery_read_task(
    #[cfg(feature = "esp32c3")] adc_pin: GpioPin<2>,

    #[cfg(feature = "esp32")] adc_pin: GpioPin<34>,

    adc: esp_hal::peripherals::ADC1,
    state: crate::state::GlobalState,
) {
    let mut adc_config = AdcConfig::new();

    #[cfg(feature = "esp32c3")]
    let mut adc_pin = adc_config.enable_pin_with_cal::<_, AdcCal>(adc_pin, Attenuation::_11dB);

    #[cfg(feature = "esp32")]
    let mut adc_pin = adc_config.enable_pin(adc_pin, Attenuation::_11dB);

    let mut adc = Adc::new(adc, adc_config);

    let mut battery_start = Instant::now();

    let base_freq = 2.0;
    let sample_freq = 1000.0;
    let sensitivity = 0.5;
    let mut smoother = dyn_smooth::DynamicSmootherEcoF32::new(base_freq, sample_freq, sensitivity);
    let mut avg = RollingAverage::<128>::new();
    let mut lcd_sent = false;

    loop {
        Timer::after_millis(100).await;
        let read = macros::nb_to_fut!(adc.read_oneshot(&mut adc_pin))
            .await
            .unwrap_or(0);

        if !lcd_sent {
            state
                .show_battery
                .signal(bat_percentage(calculate(read as f64)));

            lcd_sent = true;
        }

        let read = smoother.tick(read as f32);
        avg.push(read);

        #[cfg(feature = "bat_dev_lcd")]
        {
            let mut state = state.state.lock().await;
            state.current_bat_read = Some(read);
        }

        if (Instant::now() - battery_start).as_millis() < BATTERY_SEND_INTERVAL_MS {
            continue;
        }

        battery_start = Instant::now();
        let bat_calc_mv = calculate(read as f64);
        let bat_percentage = bat_percentage(bat_calc_mv);

        crate::ws::send_packet(crate::structs::TimerPacket {
            tag: None,
            data: crate::structs::TimerPacketInner::Battery {
                level: Some(bat_percentage as f64),
                voltage: Some(bat_calc_mv / 1000.0),
            },
        })
        .await;

        log::info!("calc({read}): {bat_calc_mv}mV {bat_percentage}%");
        #[cfg(feature = "bat_dev_lcd")]
        {
            let mut state = state.state.lock().await;
            state.avg_bat_read = avg.average();
        }
    }
}

fn interpolate(v1: f64, p1: u8, v2: f64, p2: u8, voltage: f64) -> u8 {
    let percentage = p1 as f64 + (voltage - v1) * (p2 as f64 - p1 as f64) / (v2 - v1);
    percentage as u8
}

fn bat_percentage(mv: f64) -> u8 {
    if mv <= BAT_MIN {
        return 0;
    }
    if mv >= BAT_MAX {
        return 100;
    }

    // Find the two closest voltage points in our curve
    for window in BATTERY_CURVE.windows(2) {
        let (v1, p1) = window[0];
        let (v2, p2) = window[1];

        if mv >= v1 && mv <= v2 {
            return interpolate(v1, p1, v2, p2, mv);
        }
    }

    // Fallback to linear interpolation if something goes wrong
    ((mv - BAT_MIN) / (BAT_MAX - BAT_MIN) * 100.0) as u8
}

#[cfg(feature = "esp32")]
fn calculate(x: f64) -> f64 {
    6827.85 - 1.56797 * x
}

#[cfg(feature = "esp32c3")]
fn calculate(x: f64) -> f64 {
    0.0
}
