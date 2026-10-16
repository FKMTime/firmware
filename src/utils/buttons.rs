use esp_hal::gpio::Input;

#[cfg(feature = "v3")]
use embassy_time::Instant;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Button {
    First,
    Second,
    Third,
    Fourth,
}

impl Button {
    /// Single-button mask to button (`None` for no or multiple buttons).
    pub fn from_mask(mask: u8) -> Option<Self> {
        match mask {
            0b0001 => Some(Self::First),
            0b0010 => Some(Self::Second),
            0b0100 => Some(Self::Third),
            0b1000 => Some(Self::Fourth),
            _ => None,
        }
    }
}

/// The four buttons, sampled as a bit mask (bit 0 = first button).
pub struct ButtonInputs {
    #[cfg(feature = "v4")]
    inputs: [Input<'static>; 4],
    #[cfg(feature = "v3")]
    input: Input<'static>,
    #[cfg(feature = "v3")]
    reg: adv_shift_registers::wrappers::ShifterValue,

    mask: u8,
    /// v3: a changed mask waiting to be stable for `BUTTON_DEBOUNCE_MS`.
    #[cfg(feature = "v3")]
    pending: Option<(u8, Instant)>,
}

impl ButtonInputs {
    #[cfg(feature = "v4")]
    pub fn new(inputs: [Input<'static>; 4]) -> Self {
        Self { inputs, mask: 0 }
    }

    #[cfg(feature = "v3")]
    pub fn new(input: Input<'static>, reg: adv_shift_registers::wrappers::ShifterValue) -> Self {
        Self {
            input,
            reg,
            mask: 0,
            pending: None,
        }
    }

    fn sample(&self) -> u8 {
        // esp-emu reads every input pin high (no pulls): sim presses only come from stdin
        if cfg!(feature = "sim") {
            return 0;
        }

        let mut mask = 0;

        #[cfg(feature = "v4")]
        for (i, btn) in self.inputs.iter().enumerate() {
            if btn.is_high() {
                mask |= 1 << i;
            }
        }

        #[cfg(feature = "v3")]
        for i in 0..4 {
            self.reg.set_value(0b1000_0000 >> i);
            if self.input.is_high() {
                mask |= 1 << i;
            }
        }

        mask
    }

    /// Samples the buttons (OR-ed with `extra`, e.g. simulated e2e presses) and returns
    /// `(previous, new)` mask when it changed.
    pub fn poll(&mut self, extra: u8) -> Option<(u8, u8)> {
        let new = self.sample() | extra;

        // v3: accept a change only once it is stable for BUTTON_DEBOUNCE_MS (filters bounce and
        // shift register scan glitches). v4 has per-pin pull-downs, so edges are taken as is.
        #[cfg(feature = "v3")]
        {
            if new == self.mask {
                self.pending = None;
                return None;
            }
            match self.pending {
                Some((cand, since)) if cand == new => {
                    if since.elapsed().as_millis() < crate::consts::BUTTON_DEBOUNCE_MS {
                        return None;
                    }
                    self.pending = None;
                }
                _ => {
                    self.pending = Some((new, Instant::now()));
                    return None;
                }
            }
        }

        if new == self.mask {
            return None;
        }
        let prev = core::mem::replace(&mut self.mask, new);
        Some((prev, new))
    }
}
