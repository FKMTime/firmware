pub struct RollingAverage<const N: usize> {
    buffer: [f32; N],
    sum: f32,
    position: usize,
    filled: bool,
}

#[allow(dead_code)]
impl<const N: usize> RollingAverage<N> {
    pub const fn new() -> Self {
        Self {
            buffer: [0.0; N],
            sum: 0.0,
            position: 0,
            filled: false,
        }
    }

    pub fn push(&mut self, value: f32) {
        // Subtract the old value from sum before it's overwritten
        self.sum -= self.buffer[self.position];
        // Add new value
        self.buffer[self.position] = value;
        self.sum += value;

        // Update position
        self.position = (self.position + 1) % N;
        if self.position == 0 {
            self.filled = true;
        }
    }

    pub fn average(&self) -> Option<f32> {
        if !self.filled && self.position == 0 {
            return None;
        }
        let count = if self.filled { N } else { self.position };
        Some(self.sum / count as f32)
    }

    pub fn is_filled(&self) -> bool {
        self.filled
    }

    pub fn clear(&mut self) {
        self.buffer = [0.0; N];
        self.sum = 0.0;
        self.position = 0;
        self.filled = false;
    }
}
