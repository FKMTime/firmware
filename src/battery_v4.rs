use crate::{consts::BATTERY_SEND_INTERVAL_MS, utils::shared_i2c::SharedI2C};
use embassy_time::{Instant, Timer};

const READ_INTERVAL_MS: u64 = 100;
const MAX_RETRY_MS: u64 = 30_000;


#[embassy_executor::task]
pub async fn battery_read_task(i2c: SharedI2C, state: crate::state::GlobalState) {
    let Ok(mut gauge) = bq27441::Bq27441Async::new(i2c).await else {
        log::error!("BQ27441 init failed!");
        crate::utils::error_log::add_error(crate::utils::error_log::codes::BATTERY_INIT_FAILED);
        return;
    };

    if let Ok(soc) = gauge.state_of_charge().await
        && soc == 0
    {
        log::warn!("Battery was removed before boot!");
    }

    let mut last_soc = 0;
    let mut last_charging = true;
    let mut last_sent = Instant::now();
    let mut outage = false;
    let mut retry_ms = READ_INTERVAL_MS;
    loop {
        if crate::power::is_sleeping() {
            Timer::after_millis(500).await;
            continue;
        }

        let soc = read(gauge.state_of_charge()).await;
        let mv = read(gauge.voltage()).await;
        let ma = read(gauge.average_current()).await;
        let (Some(soc), Some(mv), Some(ma)) = (soc, mv, ma) else {
            // a missing/broken gauge must not flood the error log or the shared i2c bus
            if !outage {
                outage = true;
                log::warn!("Battery gauge not responding");
                crate::utils::error_log::add_error(
                    crate::utils::error_log::codes::BATTERY_I2C_TIMEOUT,
                );
            }
            retry_ms = (retry_ms * 2).min(MAX_RETRY_MS);
            Timer::after_millis(retry_ms).await;
            continue;
        };
        if outage {
            outage = false;
            log::info!("Battery gauge responding again");
        }
        retry_ms = READ_INTERVAL_MS;

        let mv = mv as f64;
        let soc = match soc as u8 {
            0 => crate::utils::battery::percent(crate::utils::battery::adc_to_mv(mv)),
            soc => soc,
        };
        let charging = ma >= 0;

        if last_soc != soc || last_charging != charging {
            state.battery.write(|b| {
                b.percent = soc;
                b.charging = charging;
            });
            state.battery_ready.signal(());
            last_soc = soc;
            last_charging = charging;
        }

        if last_sent.elapsed().as_millis() >= BATTERY_SEND_INTERVAL_MS {
            if state.status.read(|s| s.server_connected) == Some(true) {
                _ = crate::ws::send(crate::structs::TimerPacket {
                    tag: None,
                    data: crate::structs::TimerPacketInner::Battery {
                        level: Some(soc as f64),
                        voltage: Some(mv),
                    },
                });
            }

            log::info!("Battery {mv}mv {soc}% (avg current: {ma}mA)");
            last_sent = Instant::now();
        }

        Timer::after_millis(READ_INTERVAL_MS).await;
    }
}

/// One gauge read; `None` on error or timeout.
async fn read<T, E>(fut: impl core::future::Future<Output = Result<T, E>>) -> Option<T> {
    embassy_time::with_timeout(embassy_time::Duration::from_millis(1500), fut)
        .await
        .ok()?
        .ok()
}
