//! Shared battery voltage curve and percentage helpers (identical on v3/v4).

pub const BATTERY_CURVE: [(f64, u8); 11] = [
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
pub const BAT_MIN: f64 = BATTERY_CURVE[0].0;
pub const BAT_MAX: f64 = BATTERY_CURVE[BATTERY_CURVE.len() - 1].0;

pub fn interpolate(v1: f64, p1: u8, v2: f64, p2: u8, voltage: f64) -> u8 {
    let percentage = p1 as f64 + (voltage - v1) * (p2 as f64 - p1 as f64) / (v2 - v1);
    percentage as u8
}

pub fn bat_percentage(mv: f64) -> u8 {
    if mv <= BAT_MIN {
        return 0;
    }
    if mv >= BAT_MAX {
        return 100;
    }

    for window in BATTERY_CURVE.windows(2) {
        let (v1, p1) = window[0];
        let (v2, p2) = window[1];

        if mv >= v1 && mv <= v2 {
            return interpolate(v1, p1, v2, p2, mv);
        }
    }

    // Unreachable for valid input (NaN guard).
    ((mv - BAT_MIN) / (BAT_MAX - BAT_MIN) * 100.0) as u8
}

pub fn calculate(x: f64) -> f64 {
    1.69874 * x + 66.6103
}
