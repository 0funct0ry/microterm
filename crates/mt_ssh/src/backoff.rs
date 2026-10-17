//! Reconnect backoff (SPEC §11.4): 1, 2, 4, 8, 16, 30, 30 … seconds with ±20 % jitter.

use std::time::Duration;

const SCHEDULE: [u64; 6] = [1, 2, 4, 8, 16, 30];

/// A tiny xorshift generator: jitter needs no cryptographic randomness.
#[derive(Debug, Clone)]
pub struct Jitter(u64);

impl Jitter {
    pub fn new(seed: u64) -> Self {
        Jitter(seed | 1)
    }

    pub fn from_clock() -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Self::new(n ^ (std::process::id() as u64) << 32)
    }

    /// A value in `[-1.0, 1.0]`.
    pub fn next_unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

#[derive(Debug, Clone)]
pub struct Backoff {
    attempt: u32,
    jitter: Jitter,
}

impl Backoff {
    pub fn new(jitter: Jitter) -> Self {
        Backoff { attempt: 0, jitter }
    }

    /// Attempts started so far (the number shown in the overlay).
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Starts the next attempt and returns how long to wait before making it. The first wait
    /// is the base 1 s: the connection has just been lost.
    pub fn next_delay(&mut self) -> Duration {
        let base = SCHEDULE[(self.attempt as usize).min(SCHEDULE.len() - 1)];
        self.attempt += 1;
        let factor = 1.0 + 0.2 * self.jitter.next_unit();
        Duration::from_secs_f64(base as f64 * factor)
    }

    /// The connection came back.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_is_1_2_4_8_16_30_30_within_20_percent() {
        let mut b = Backoff::new(Jitter::new(42));
        let want = [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0];
        for (i, w) in want.iter().enumerate() {
            let d = b.next_delay().as_secs_f64();
            assert!(
                d >= w * 0.8 - 1e-9 && d <= w * 1.2 + 1e-9,
                "step {i}: {d} vs {w}"
            );
            assert_eq!(b.attempt(), i as u32 + 1);
        }
    }

    #[test]
    fn jitter_covers_the_range_and_reset_restarts() {
        let mut j = Jitter::new(7);
        let xs: Vec<f64> = (0..2000).map(|_| j.next_unit()).collect();
        assert!(xs.iter().all(|x| (-1.0..=1.0).contains(x)));
        assert!(xs.iter().any(|x| *x < -0.8) && xs.iter().any(|x| *x > 0.8));
        let mut b = Backoff::new(Jitter::new(1));
        b.next_delay();
        b.next_delay();
        b.reset();
        assert_eq!(b.attempt(), 0);
        assert!(b.next_delay().as_secs_f64() <= 1.2 + 1e-9);
    }
}
