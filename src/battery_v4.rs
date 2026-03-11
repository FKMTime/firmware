use crate::{consts::BATTERY_SEND_INTERVAL_MS, state::sleep_state, utils::shared_i2c::SharedI2C};
use embassy_time::Timer;

const BATTERY_CURVE: [(f64, u8); 11] = [
    (3350.0, 0),
    (3400.0, 13),
    (3450.0, 19),
    (3500.0, 25),
    (3550.0, 31),
    (3600.0, 38),
    (3700.0, 50),
    (3800.0, 63),
    (3900.0, 75),
    (4000.0, 88),
    (4100.0, 100),
];
const BAT_MIN: f64 = BATTERY_CURVE[0].0;
const BAT_MAX: f64 = BATTERY_CURVE[BATTERY_CURVE.len() - 1].0;

#[embassy_executor::task]
pub async fn battery_read_task(i2c: SharedI2C, state: crate::state::GlobalState) {
    let Ok(mut gauge) = bq27441::Bq27441Async::new(i2c).await else {
        state.show_battery.signal(0);
        log::error!("BQ27441 init failed!");
        return;
    };

    let mut lcd_sent = false;
    loop {
        if sleep_state() {
            Timer::after_millis(500).await;
            continue;
        }

        if !lcd_sent {
            let mut soc = gauge.state_of_charge().await.unwrap_or(0) as u8;
            if soc == 0 {
                let volt = gauge.voltage().await.unwrap_or(0) as f64;
                soc = bat_percentage(calculate(volt));
            }

            state.show_battery.signal(soc);
            lcd_sent = true;
            Timer::after_millis(BATTERY_SEND_INTERVAL_MS).await;
            continue;
        }

        let mut soc = gauge.state_of_charge().await.unwrap_or(0) as u8;
        let mv = gauge.voltage().await.unwrap_or(0) as f64;
        if soc == 0 {
            soc = bat_percentage(calculate(mv));
        }
        state.show_battery.signal(soc);

        if state.state.lock().await.server_connected == Some(true) {
            crate::ws::send_packet(crate::structs::TimerPacket {
                tag: None,
                data: crate::structs::TimerPacketInner::Battery {
                    level: Some(soc as f64),
                    voltage: Some(mv),
                },
            })
            .await;
        }

        log::info!("Battery {mv}mv {soc}%");
        Timer::after_millis(BATTERY_SEND_INTERVAL_MS).await;
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

fn calculate(x: f64) -> f64 {
    1.69874 * x + 66.6103
}
