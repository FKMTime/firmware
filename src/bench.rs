//! On-device timing: `cargo build -r --features bench` logs min/avg/max (µs) every 5 s.
//! Without the feature the [`bench!`] hooks expand to nothing (the image is unchanged).

/// `bench!(STAT)` times the rest of the enclosing scope; `bench!(STAT, us)` records a value.
#[macro_export]
macro_rules! bench {
    ($stat:ident) => {
        #[cfg(feature = "bench")]
        let _bench = $crate::bench::Span::start($crate::bench::Stat::$stat);
    };
    ($stat:ident, $us:expr) => {
        #[cfg(feature = "bench")]
        $crate::bench::record($crate::bench::Stat::$stat, $us);
    };
}

#[cfg(feature = "bench")]
pub use imp::*;

#[cfg(feature = "bench")]
mod imp {
    use core::cell::Cell;
    use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

    #[derive(Clone, Copy)]
    #[allow(non_camel_case_types, dead_code)] // which ones are used depends on the hw features
    pub enum Stat {
        STACKMAT_LOOP,
        FRAME_PUBLISH,
        PADS_EVENT,
        RFID_POLL,
        LCD_FRAME,
    }
    const NAMES: [&str; 5] = ["stackmat loop", "frame→publish", "pads event→state", "rfid poll", "lcd frame"];

    #[derive(Clone, Copy)]
    struct Acc { min: u64, max: u64, sum: u64, n: u64 }
    const EMPTY: Acc = Acc { min: u64::MAX, max: 0, sum: 0, n: 0 };

    /// Accumulators and the time of the last report.
    static ACCS: Mutex<CriticalSectionRawMutex, Cell<([Acc; 5], u64)>> =
        Mutex::new(Cell::new(([EMPTY; 5], 0)));

    fn now_us() -> u64 {
        embassy_time::Instant::now().as_micros()
    }
    pub fn record(stat: Stat, us: u64) {
        let now = now_us();
        let report = ACCS.lock(|cell| {
            let (mut accs, last) = cell.get();
            let a = &mut accs[stat as usize];
            (a.min, a.max, a.sum, a.n) = (a.min.min(us), a.max.max(us), a.sum + us, a.n + 1);
            let due = now.saturating_sub(last) >= 5_000_000;
            cell.set(if due { ([EMPTY; 5], now) } else { (accs, last) });
            due.then_some(accs)
        });

        for (name, a) in NAMES.iter().zip(report.unwrap_or([EMPTY; 5])) {
            if a.n > 0 {
                log::info!("bench {name}: min {} avg {} max {} µs (n={})", a.min, a.sum / a.n, a.max, a.n);
            }
        }
    }

    /// Records the time from [`Span::start`] to drop.
    pub struct Span(Stat, u64);

    impl Span {
        pub fn start(stat: Stat) -> Self {
            Self(stat, now_us())
        }
    }
    impl Drop for Span {
        fn drop(&mut self) {
            record(self.0, now_us().saturating_sub(self.1));
        }
    }
}
