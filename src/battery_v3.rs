use crate::{
    consts::BATTERY_SEND_INTERVAL_MS,
    state::sleep_state,
    utils::{
        battery::{bat_percentage, calculate},
        rolling_average::RollingAverage,
    },
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};

type AdcCal = esp_hal::analog::adc::AdcCalCurve<esp_hal::peripherals::ADC1<'static>>;

#[embassy_executor::task]
pub async fn battery_read_task(
    adc_pin: esp_hal::peripherals::GPIO2<'static>,
    adc: esp_hal::peripherals::ADC1<'static>,
    state: crate::state::GlobalState,
) {
    let mut adc_config = AdcConfig::new();

    let mut adc_pin = adc_config.enable_pin_with_cal::<_, AdcCal>(adc_pin, Attenuation::_11dB);
    let mut adc = Adc::new(adc, adc_config).into_async();

    let mut battery_start = Instant::now().saturating_add(Duration::from_millis(300));

    let base_freq = 2.0;
    let sample_freq = 1000.0;
    let sensitivity = 0.5;
    let mut smoother = dyn_smooth::DynamicSmootherEcoF32::new(base_freq, sample_freq, sensitivity);
    let mut avg = RollingAverage::<128>::new();
    let mut lcd_sent = false;

    let mut sample_rate_millis = 10;
    loop {
        Timer::after_millis(sample_rate_millis).await;
        if sleep_state() {
            Timer::after_millis(500).await;
            continue;
        }

        let read = adc.read_oneshot(&mut adc_pin).await;
        let read = smoother.tick(read as f32);
        avg.push(read);

        #[cfg(feature = "bat_dev_lcd")]
        {
            state.battery.lock().await.current_bat_read = Some(read);
            state.state.signal();
        }

        let now = Instant::now();
        if !lcd_sent && battery_start <= now {
            state
                .show_battery
                .signal(bat_percentage(calculate(read as f64)));

            lcd_sent = true;
            sample_rate_millis = 100;
        }

        if battery_start > now || (now - battery_start).as_millis() < BATTERY_SEND_INTERVAL_MS {
            continue;
        }

        battery_start = Instant::now();
        let bat_calc_mv = calculate(read as f64);
        let bat_percentage = bat_percentage(bat_calc_mv);

        if state.state.lock_silent().await.conn.server_connected == Some(true) {
            crate::ws::send_packet(crate::structs::TimerPacket {
                tag: None,
                data: crate::structs::TimerPacketInner::Battery {
                    level: Some(bat_percentage as f64),
                    voltage: Some(bat_calc_mv / 1000.0),
                },
            })
            .await;
        }

        log::info!("calc({read}): {bat_calc_mv}mV {bat_percentage}%");
        #[cfg(feature = "bat_dev_lcd")]
        {
            state.battery.lock().await.avg_bat_read = avg.average();
            state.state.signal();
        }
    }
}
