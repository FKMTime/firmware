//! Battery voltage → percent (shared by v3 and v4, host-tested in host-tests/).

/// LiPo discharge curve: (millivolts, percent), ascending.
const CURVE: [(f64, u8); 11] = [
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

/// Percent for `mv`, linearly interpolated on [`CURVE`] (clamped to 0..=100).
pub fn percent(mv: f64) -> u8 {
    if mv <= CURVE[0].0 {
        return 0;
    }
    for w in CURVE.windows(2) {
        let ((v1, p1), (v2, p2)) = (w[0], w[1]);
        if mv <= v2 {
            return (p1 as f64 + (mv - v1) * (p2 - p1) as f64 / (v2 - v1)) as u8;
        }
    }
    100
}

/// v3 ADC reading (after smoothing) → battery millivolts (calibrated divider).
#[cfg(any(feature = "v3", test))]
pub fn adc_to_mv(raw: f64) -> f64 {
    1.69874 * raw + 66.6103
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve() {
        assert_eq!(percent(3000.0), 0);
        assert_eq!(percent(3350.0), 0);
        assert_eq!(percent(3375.0), 6);
        assert_eq!(percent(3400.0), 13);
        assert_eq!(percent(3650.0), 44);
        assert_eq!(percent(4100.0), 100);
        assert_eq!(percent(5000.0), 100);
    }
}
