use crate::{
    consts::BATTERY_SEND_INTERVAL_MS,
    state::sleep_state,
    utils::{
        battery::{bat_percentage, calculate},
        shared_i2c::SharedI2C,
    },
};
use embassy_time::{Instant, Timer};

#[embassy_executor::task]
pub async fn battery_read_task(i2c: SharedI2C, state: crate::state::GlobalState) {
    let Ok(mut gauge) = bq27441::Bq27441Async::new(i2c).await else {
        log::error!("BQ27441 init failed!");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::BATTERY_INIT_FAILED)
            .await;
        return;
    };

    if let Ok(soc) = gauge.state_of_charge().await
        && soc == 0
    {
        log::warn!("Battery was removed before boot!");
    }

    let mut last_soc = 0;
    let mut last_charging = true;
    let mut last_mv = 0.0;
    let mut last_ma = 0;
    let mut last_sent = Instant::now();
    loop {
        if sleep_state() {
            Timer::after_millis(500).await;
            continue;
        }

        let mut soc = last_soc;
        match embassy_time::with_timeout(
            embassy_time::Duration::from_millis(1500),
            gauge.state_of_charge(),
        )
        .await
        {
            Ok(Ok(val)) => soc = val as u8,
            _ => {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::BATTERY_I2C_TIMEOUT,
                )
                .await;
            }
        };
        let mut mv = last_mv;
        match embassy_time::with_timeout(embassy_time::Duration::from_millis(1500), gauge.voltage())
            .await
        {
            Ok(Ok(val)) => mv = val as f64,
            _ => {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::BATTERY_I2C_TIMEOUT,
                )
                .await;
            }
        };
        if soc == 0 {
            soc = bat_percentage(calculate(mv));
        }
        let mut ma = last_ma;
        match embassy_time::with_timeout(
            embassy_time::Duration::from_millis(1500),
            gauge.average_current(),
        )
        .await
        {
            Ok(Ok(val)) => ma = val,
            _ => {
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::BATTERY_I2C_TIMEOUT,
                )
                .await;
            }
        };
        let charging = ma >= 0;

        if last_soc != soc || last_charging != charging {
            state.battery.lock().await.battery_status = (soc, charging);
            state.show_battery.signal(soc);
            last_soc = soc;
            last_charging = charging;
        }

        if last_sent.elapsed().as_millis() >= BATTERY_SEND_INTERVAL_MS {
            if state.state.lock_silent().await.conn.server_connected == Some(true) {
                crate::ws::send_packet(crate::structs::TimerPacket {
                    tag: None,
                    data: crate::structs::TimerPacketInner::Battery {
                        level: Some(soc as f64),
                        voltage: Some(mv),
                    },
                })
                .await;
            }

            log::info!("Battery {mv}mv {soc}% (avg current: {ma}mA)");
            last_sent = Instant::now();
        }

        last_mv = mv;
        last_ma = ma;

        Timer::after_millis(100).await;
    }
}
