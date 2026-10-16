//! Stackmat signal decoding and timing (no deps, host-tested in `host-tests`).
//!
//! The timer sends frames back to back at 1200 baud 8N1: a state byte, the time digits,
//! a checksum (64 + digit sum), `\n`, `\r`. Gen 4/5 timers send 6 digits (`M SS mmm`),
//! gen 3 timers 5 digits (`M SS cc`).

// timer-func boards time with pads; the stackmat decoder is unused there
#![cfg_attr(feature = "timer-func", allow(dead_code))]

/// One byte on the wire (10 bits at 1200 baud), in µs.
pub const BYTE_US: u64 = 10 * 1_000_000 / 1200;
/// A frame is decoded 8 bytes after the timer sampled its time (both generations).
pub const FRAME_RX_DELAY_US: u64 = 8 * BYTE_US;
/// No valid frame for this long = disconnected.
pub const DISCONNECT_US: u64 = 500_000;

/// Largest time a stackmat frame can carry (9:59.999).
#[cfg(any(all(feature = "e2e", not(feature = "timer-func")), test))]
const MAX_FRAME_MS: u64 = 9 * 60_000 + 59_999;

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum StackmatTimerState {
    Unknown,
    Reset,
    Running,
    Stopped,
}

impl StackmatTimerState {
    pub fn from_u8(val: u8) -> Self {
        match val {
            b'I' => Self::Reset,
            b' ' => Self::Running,
            b'S' => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    #[cfg(any(all(feature = "e2e", not(feature = "timer-func")), test))]
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Reset => b'I',
            Self::Running => b' ',
            Self::Stopped => b'S',
        }
    }
}

/// A decoded frame: timer state and the time it shows (ms).
pub type Frame = (StackmatTimerState, u64);

/// Parses a gen4/5 frame (`state`, 6 digits, checksum).
#[cfg(test)]
pub fn parse_stackmat_data(data: &[u8; 8]) -> Result<Frame, ()> {
    parse(data, 1).ok_or(())
}

/// `state` + digits + checksum; `scale` turns the last digit group into ms (1 or 10).
fn parse(data: &[u8], scale: u64) -> Option<Frame> {
    let (&state, rest) = data.split_first()?;
    let (&sum, digits) = rest.split_last()?;
    if checksum(digits) != sum || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }

    let num = |d: &[u8]| d.iter().fold(0, |acc, &x| acc * 10 + (x - b'0') as u64);
    let ms = num(&digits[..1]) * 60_000 + num(&digits[1..3]) * 1000 + num(&digits[3..]) * scale;

    let mut state = StackmatTimerState::from_u8(state);
    if ms > 0 && state == StackmatTimerState::Reset {
        state = StackmatTimerState::Stopped;
    }
    Some((state, ms))
}

/// 64 + sum of the time digits (garbage bytes just make it not match).
fn checksum(digits: &[u8]) -> u8 {
    digits
        .iter()
        .fold(64u8, |acc, &x| acc.wrapping_add(x.wrapping_sub(b'0')))
}

/// Byte stream → frames. Resyncs on its own after garbage or partial frames.
#[derive(Default)]
pub struct Parser {
    /// Last 8 bytes of the current line (sliding window).
    window: [u8; 8],
    /// Bytes since the last line end.
    count: usize,
    emitted: bool,
    /// A gen4 timer was seen: a 7-byte line is then a gen4 frame that lost a byte, not gen3.
    gen4: bool,
}

impl Parser {
    pub fn push(&mut self, byte: u8) -> Option<Frame> {
        match byte {
            0 => None,
            b'\n' | b'\r' => {
                // gen3 frames are 7 bytes: only complete at the line end
                let frame = (self.count == 7 && !self.emitted && !self.gen4)
                    .then(|| parse(&self.window[1..], 10))
                    .flatten();
                self.count = 0;
                self.emitted = false;
                frame
            }
            _ => {
                self.window.copy_within(1.., 0);
                self.window[7] = byte;
                self.count += 1;
                // gen4/5: decode at the checksum byte (no need to wait for the line end)
                let frame = (self.count >= 8).then(|| parse(&self.window, 1)).flatten();
                self.emitted |= frame.is_some();
                self.gen4 |= frame.is_some();
                frame
            }
        }
    }
}

/// What a confirmed state change means for the solve.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum Event {
    /// Timer started; `at` = when it read 0 (same clock as the `now` passed in).
    Start { at: u64 },
    /// Timer stopped showing this time (ms).
    Stop(u64),
    Reset,
}

/// Turns frames into solve events. Times are µs on the caller's clock.
#[derive(Default)]
pub struct Tracker {
    rx_delay_us: u64,
    last_state: Option<StackmatTimerState>,
    confirmed: Option<StackmatTimerState>,
    /// When the last valid frame's time was sampled, and that time (ms).
    last_frame: Option<(u64, u64)>,
}

impl Tracker {
    /// `rx_delay_us`: time between the timer sampling a frame and it being decoded
    /// ([`FRAME_RX_DELAY_US`] on the wire, 0 for synthetic frames).
    pub fn new(rx_delay_us: u64) -> Self {
        Self {
            rx_delay_us,
            ..Self::default()
        }
    }

    /// A frame decoded at `now`. A state change counts once two frames in a row agree
    /// (one corrupted-but-valid frame can't start or stop a solve).
    pub fn frame(&mut self, now: u64, (state, ms): Frame) -> Option<Event> {
        let sampled = now.saturating_sub(self.rx_delay_us);
        self.last_frame = Some((sampled, ms));

        let repeated = self.last_state.replace(state) == Some(state);
        if !repeated || self.confirmed == Some(state) || state == StackmatTimerState::Unknown {
            return None;
        }
        self.confirmed = Some(state);
        Some(match state {
            StackmatTimerState::Running => Event::Start {
                at: sampled.saturating_sub(ms * 1000),
            },
            StackmatTimerState::Stopped => Event::Stop(ms),
            _ => Event::Reset,
        })
    }

    pub fn connected(&self, now: u64) -> bool {
        self.last_frame
            .is_some_and(|(at, _)| now.saturating_sub(at) < DISCONNECT_US + self.rx_delay_us)
    }

    /// While a solve runs: the time it shows now (ms), extrapolated from the last frame.
    pub fn running_ms(&self, now: u64) -> Option<u64> {
        let (at, ms) = self.last_frame?;
        (self.confirmed == Some(StackmatTimerState::Running))
            .then(|| ms + now.saturating_sub(at) / 1000)
    }

    /// The solve ended outside of the timer (limit, button stop, DNF): stop extrapolating.
    pub fn end_solve(&mut self) {
        if self.confirmed == Some(StackmatTimerState::Running) {
            self.confirmed = None;
        }
    }
}

/// Builds a gen4 frame (e2e simulation); times above 9:59.999 are clamped.
#[cfg(any(all(feature = "e2e", not(feature = "timer-func")), test))]
pub fn generate_stackmat_data(state: StackmatTimerState, time_ms: u64) -> [u8; 8] {
    let time_ms = time_ms.min(MAX_FRAME_MS);
    let digit = |v: u64| b'0' + (v % 10) as u8;

    let mut data = [0u8; 8];
    data[0] = state.to_u8();
    data[1] = digit(time_ms / 60_000);
    data[2] = digit(time_ms % 60_000 / 10_000);
    data[3] = digit(time_ms % 10_000 / 1000);
    data[4] = digit(time_ms % 1000 / 100);
    data[5] = digit(time_ms % 100 / 10);
    data[6] = digit(time_ms);
    data[7] = checksum(&data[1..7]);
    data
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;
    use StackmatTimerState::*;

    fn feed(parser: &mut Parser, bytes: &[u8]) -> Vec<Frame> {
        bytes.iter().filter_map(|&b| parser.push(b)).collect()
    }

    fn gen4_line(state: StackmatTimerState, ms: u64) -> Vec<u8> {
        let mut line = generate_stackmat_data(state, ms).to_vec();
        line.extend_from_slice(b"\n\r");
        line
    }

    fn gen3_line(state: StackmatTimerState, cs: u64) -> Vec<u8> {
        let d = |v: u64| b'0' + (v % 10) as u8;
        let digits = [d(cs / 6000), d(cs % 6000 / 1000), d(cs % 1000 / 100), d(cs % 100 / 10), d(cs)];
        let mut line = std::vec![state.to_u8()];
        line.extend_from_slice(&digits);
        line.push(checksum(&digits));
        line.extend_from_slice(b"\n\r");
        line
    }

    #[test]
    fn frame_round_trip() {
        for (state, ms) in [(Running, 0), (Running, 1), (Stopped, 12_345), (Running, 61_005), (Stopped, MAX_FRAME_MS)] {
            assert_eq!(parse_stackmat_data(&generate_stackmat_data(state, ms)), Ok((state, ms)));
        }
        // used to emit ':' as the minute digit from 10 minutes on
        let frame = generate_stackmat_data(Running, 11 * 60_000);
        assert_eq!(parse_stackmat_data(&frame), Ok((Running, MAX_FRAME_MS)));
        let frame = generate_stackmat_data(Reset, 500);
        assert_eq!(parse_stackmat_data(&frame), Ok((Stopped, 500)));
    }

    #[test]
    fn rejects_bad_frames() {
        let mut frame = generate_stackmat_data(Running, 1234);
        frame[7] ^= 1;
        assert_eq!(parse_stackmat_data(&frame), Err(()));
        assert_eq!(parse_stackmat_data(b"I0:00000"), Err(()));
        assert_eq!(parse_stackmat_data(&[0xFF; 8]), Err(()));
        assert_eq!(parse_stackmat_data(&[0; 8]), Err(()));
    }

    #[test]
    fn parser_both_generations_and_resync() {
        let mut p = Parser::default();
        let mut stream = gen4_line(Running, 1234);
        stream.extend_from_slice(b"\xFF\x13garbage ");
        stream.extend(gen4_line(Stopped, 9_876));
        stream.extend_from_slice(b" 12\n\r"); // partial frame
        stream.extend(gen4_line(Reset, 0).iter().flat_map(|&b| [b, 0])); // NULs in between
        assert_eq!(feed(&mut p, &stream), [(Running, 1234), (Stopped, 9_876), (Reset, 0)]);

        // gen4 frame that lost a '0' digit: valid gen3 checksum, but ignored after gen4
        let mut lost = gen4_line(Running, 12_345);
        lost.remove(1);
        assert!(feed(&mut p, &lost).is_empty());

        let mut gen3 = Parser::default();
        let mut stream = b"x3\n\r".to_vec();
        stream.extend(gen3_line(Running, 1_234)); // 12.34 s
        stream.extend(gen3_line(Stopped, 59_999));
        assert_eq!(feed(&mut gen3, &stream), [(Running, 12_340), (Stopped, 599_990)]);

        let mut bad = gen4_line(Running, 500);
        bad[3] = b'x';
        assert!(feed(&mut p, &bad).is_empty());
        assert_eq!(feed(&mut p, &gen4_line(Running, 501)), [(Running, 501)]);
    }

    /// A simulated stackmat on a UART: returns (byte, arrival µs) for frames sent back to back
    /// (+ jitter), with `frame(t)` giving the state/time shown at `t` (None = cable unplugged).
    fn wire(until: u64, frame: impl Fn(u64) -> Option<Frame>, seed: u64) -> Vec<(u8, u64)> {
        let mut rng = seed;
        let mut rand = move |n: u64| {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (rng >> 33) % n
        };
        let (mut out, mut t) = (Vec::new(), 0);
        while t < until {
            let Some((state, ms)) = frame(t) else {
                t += 10_000;
                continue;
            };
            let mut line = gen4_line(state, ms);
            match rand(20) {
                0 => line.clear(),          // dropped frame
                1 => line[4] ^= 0x10,       // corrupted digit
                2 => line.insert(2, b'#'), // stray byte
                _ => {}
            }
            for (i, b) in line.into_iter().enumerate() {
                out.push((b, t + (i as u64 + 1) * BYTE_US));
            }
            t += 10 * BYTE_US + rand(3_000); // inter-frame gap jitter
        }
        out
    }

    /// The receive loop with the firmware's 1-byte rx threshold: bytes arrive in chunks of
    /// up to 3 (a busy executor), 1 ms late, each byte timestamped back from the chunk time
    /// like the firmware does. A pause on the wire always ends a chunk (already delivered).
    fn receive(wire: &[(u8, u64)], mut on_frame: impl FnMut(u64, Frame)) {
        let mut p = Parser::default();
        let mut chunks: Vec<Vec<(u8, u64)>> = Vec::new();
        for &(b, at) in wire {
            match chunks.last_mut() {
                Some(c) if c.len() < 3 && at - c[c.len() - 1].1 <= BYTE_US + 1_000 => c.push((b, at)),
                _ => chunks.push(std::vec![(b, at)]),
            }
        }
        for chunk in chunks {
            let now = chunk[chunk.len() - 1].1 + 1_000;
            for (i, &(b, _)) in chunk.iter().enumerate() {
                let at = now - (chunk.len() - 1 - i) as u64 * BYTE_US;
                if let Some(f) = p.push(b) {
                    on_frame(at, f);
                }
            }
        }
    }

    #[test]
    fn timing_simulation() {
        for seed in 0..50 {
            let (start, stop) = (2_000_000 + seed * 7_919, 9_000_000 + seed * 104_729);
            let solve_ms = (stop - start) / 1000;
            let shown = |t: u64| match t {
                t if t < start => Some((Reset, 0)),
                t if t < stop => Some((Running, (t - start) / 1000)),
                _ => Some((Stopped, solve_ms)),
            };

            let mut tracker = Tracker::new(FRAME_RX_DELAY_US);
            let mut events = Vec::new();
            receive(&wire(stop + 1_500_000, shown, seed), |at, f| events.extend(tracker.frame(at, f)));

            // no spurious events from dropped/corrupted frames
            assert_eq!(events.len(), 3, "seed {seed}: {events:?}");
            assert_eq!(events[0], Event::Reset);
            let Some(Event::Start { at }) = events.iter().find(|e| matches!(e, Event::Start { .. })).copied() else {
                panic!("no start: {events:?}");
            };
            // start instant within 3 ms: 1 ms digit resolution + 1 ms delivery + up to 1 ms
            // of wire jitter inside a received chunk (bytes are timestamped as contiguous)
            assert!(at.abs_diff(start) <= 3_000, "seed {seed}: start off by {} µs", at.abs_diff(start));
            // the recorded solve time is exactly what the timer shows
            assert_eq!(events[2], Event::Stop(solve_ms), "seed {seed}");
        }
    }

    #[test]
    fn disconnect_mid_solve_extrapolates() {
        let (start, unplug, replug) = (1_000_000, 4_000_000, 6_000_000);
        let shown = |t: u64| {
            (!(unplug..replug).contains(&t)).then_some((Running, t.saturating_sub(start) / 1000))
        };
        let mut tracker = Tracker::new(FRAME_RX_DELAY_US);
        let wire = wire(replug, shown, 1);
        let mut last_at = 0;
        receive(&wire, |at, f| {
            tracker.frame(at, (if at < start { Reset } else { f.0 }, f.1));
            last_at = at;
        });

        let now = 5_500_000;
        assert!(!tracker.connected(now));
        let ms = tracker.running_ms(now).expect("still running");
        assert!(ms.abs_diff((now - start) / 1000) <= 3, "{ms}");
        assert!(tracker.connected(last_at + 100_000));

        tracker.end_solve();
        assert_eq!(tracker.running_ms(now), None);
    }

    #[test]
    fn single_bogus_frame_does_not_start() {
        let mut t = Tracker::new(0);
        assert_eq!(t.frame(0, (Reset, 0)), None);
        assert_eq!(t.frame(80_000, (Reset, 0)), Some(Event::Reset));
        assert_eq!(t.frame(160_000, (Running, 20)), None); // noise
        assert_eq!(t.frame(240_000, (Reset, 0)), None);
        assert_eq!(t.frame(320_000, (Running, 0)), None);
        assert_eq!(t.frame(400_000, (Running, 80)), Some(Event::Start { at: 320_000 }));
    }

    #[test]
    fn parser_throughput() {
        let mut stream = Vec::new();
        for ms in 0..20_000 {
            stream.extend(gen4_line(Running, ms));
        }
        let mut p = Parser::default();
        let begin = std::time::Instant::now();
        let frames = stream.iter().filter(|&&b| p.push(b).is_some()).count();
        let ns = begin.elapsed().as_nanos() / frames as u128;
        assert_eq!(frames, 20_000);
        std::eprintln!("stackmat parser: {ns} ns/frame on host");
    }
}
