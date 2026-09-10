//! Reconnect backoff.
//!
//! A client that reconnects in a tight loop against a venue that is rejecting
//! it will be rate-limited or banned, which turns a transient outage into a
//! long one. Jitter matters as much as the delay: without it, every client
//! that dropped on the same venue hiccup returns at the same instant and
//! recreates the thundering herd that caused it.

use std::time::Duration;

/// Exponential backoff with full jitter.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    /// Deterministic jitter source, so tests are reproducible and the type
    /// carries no dependency on a random number generator.
    state: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_millis(500), Duration::from_secs(60))
    }
}

impl Backoff {
    /// A backoff from `base`, doubling up to `max`.
    pub fn new(base: Duration, max: Duration) -> Self {
        assert!(!base.is_zero(), "base delay must be non-zero");
        assert!(max >= base, "max delay must be at least the base");
        Self {
            base,
            max,
            attempt: 0,
            state: 0x2545_F491_4F6C_DD1D,
        }
    }

    /// Seed the jitter, so a fleet of clients does not share a delay sequence.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.state = seed | 1;
        self
    }

    /// How many failures have accumulated since the last success.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    fn next_random(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    /// The delay before the next attempt, advancing the sequence.
    ///
    /// Uses full jitter — a uniform draw from `[0, ceiling]` rather than
    /// `ceiling` itself — which spreads returning clients across the whole
    /// window instead of clustering them at its end.
    pub fn next_delay(&mut self) -> Duration {
        let shift = self.attempt.min(24);
        let ceiling_nanos = (self.base.as_nanos() as u64)
            .saturating_mul(1u64 << shift)
            .min(self.max.as_nanos() as u64);

        self.attempt = self.attempt.saturating_add(1);

        let jittered = self.next_random() % (ceiling_nanos + 1);
        Duration::from_nanos(jittered)
    }

    /// The ceiling the next delay will be drawn from, without advancing.
    pub fn ceiling(&self) -> Duration {
        let shift = self.attempt.min(24);
        let nanos = (self.base.as_nanos() as u64)
            .saturating_mul(1u64 << shift)
            .min(self.max.as_nanos() as u64);
        Duration::from_nanos(nanos)
    }

    /// Record a successful connection, returning to the base delay.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backoff() -> Backoff {
        Backoff::new(Duration::from_millis(100), Duration::from_secs(10))
    }

    #[test]
    fn the_ceiling_doubles_then_clamps() {
        let mut b = backoff();
        assert_eq!(b.ceiling(), Duration::from_millis(100));
        b.next_delay();
        assert_eq!(b.ceiling(), Duration::from_millis(200));
        b.next_delay();
        assert_eq!(b.ceiling(), Duration::from_millis(400));

        for _ in 0..20 {
            b.next_delay();
        }
        assert_eq!(b.ceiling(), Duration::from_secs(10), "must clamp at max");
    }

    #[test]
    fn every_delay_stays_within_its_ceiling() {
        let mut b = backoff();
        for _ in 0..200 {
            let ceiling = b.ceiling();
            let delay = b.next_delay();
            assert!(delay <= ceiling, "{delay:?} exceeded ceiling {ceiling:?}");
            assert!(delay <= Duration::from_secs(10));
        }
    }

    #[test]
    fn jitter_actually_varies() {
        // Without jitter, every client that dropped on the same venue hiccup
        // returns at the same instant.
        let mut b = backoff();
        for _ in 0..8 {
            b.next_delay();
        }
        let ceiling = b.ceiling();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..40 {
            let mut probe = b.clone();
            seen.insert(probe.next_delay());
            b.next_delay();
        }
        assert!(seen.len() > 1, "delays were identical at ceiling {ceiling:?}");
    }

    #[test]
    fn different_seeds_give_different_sequences() {
        let mut a = backoff().with_seed(1);
        let mut b = backoff().with_seed(2);
        let left: Vec<_> = (0..10).map(|_| a.next_delay()).collect();
        let right: Vec<_> = (0..10).map(|_| b.next_delay()).collect();
        assert_ne!(left, right);
    }

    #[test]
    fn the_same_seed_reproduces_a_sequence() {
        let mut a = backoff().with_seed(7);
        let mut b = backoff().with_seed(7);
        for _ in 0..10 {
            assert_eq!(a.next_delay(), b.next_delay());
        }
    }

    #[test]
    fn a_successful_connection_resets_the_ceiling() {
        let mut b = backoff();
        for _ in 0..6 {
            b.next_delay();
        }
        assert!(b.ceiling() > Duration::from_millis(100));
        assert_eq!(b.attempt(), 6);

        b.reset();
        assert_eq!(b.attempt(), 0);
        assert_eq!(b.ceiling(), Duration::from_millis(100));
    }

    #[test]
    fn a_long_outage_does_not_overflow() {
        // Left shifting by the attempt count overflows quickly if unclamped.
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        for _ in 0..10_000 {
            let delay = b.next_delay();
            assert!(delay <= Duration::from_secs(300));
        }
        assert_eq!(b.ceiling(), Duration::from_secs(300));
    }
}
