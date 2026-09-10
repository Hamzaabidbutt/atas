//! Timestamps.
//!
//! Every event in the system carries a nanosecond timestamp. Exchanges publish
//! milliseconds, but bar boundaries, latency measurement and replay ordering
//! all want finer resolution, and widening later would touch every struct.

use std::fmt;
use std::ops::{Add, Sub};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Nanoseconds since the Unix epoch.
///
/// `i64` nanoseconds covers years 1678–2262, which comfortably brackets any
/// market data this platform will see.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Ts(pub i64);

/// Nanoseconds in a millisecond.
pub const NANOS_PER_MILLI: i64 = 1_000_000;
/// Nanoseconds in a second.
pub const NANOS_PER_SEC: i64 = 1_000_000_000;

impl Ts {
    /// The Unix epoch.
    pub const EPOCH: Ts = Ts(0);
    /// The largest representable timestamp. Useful as an open range end.
    pub const MAX: Ts = Ts(i64::MAX);
    /// The smallest representable timestamp.
    pub const MIN: Ts = Ts(i64::MIN);

    /// Build from nanoseconds since the epoch.
    #[inline]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// Build from milliseconds since the epoch — the form exchanges send.
    ///
    /// Saturates rather than overflowing: these values come off the wire, and
    /// a venue sending a nonsense timestamp must not panic the process or,
    /// worse, wrap into a valid-looking past date in a release build.
    #[inline]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis.saturating_mul(NANOS_PER_MILLI))
    }

    /// Build from whole seconds since the epoch. Saturates like
    /// [`Ts::from_millis`].
    #[inline]
    pub const fn from_secs(secs: i64) -> Self {
        Self(secs.saturating_mul(NANOS_PER_SEC))
    }

    /// Current wall-clock time.
    ///
    /// Only for tagging locally generated events. Never use it to timestamp
    /// market data — that must carry the exchange's own timestamp, or bar
    /// boundaries drift with local clock skew.
    pub fn now() -> Self {
        let d = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch");
        Self(d.as_nanos() as i64)
    }

    /// Nanoseconds since the epoch.
    #[inline]
    pub const fn nanos(self) -> i64 {
        self.0
    }

    /// Whole milliseconds since the epoch, truncated.
    #[inline]
    pub const fn millis(self) -> i64 {
        self.0.div_euclid(NANOS_PER_MILLI)
    }

    /// Whole seconds since the epoch, truncated.
    #[inline]
    pub const fn secs(self) -> i64 {
        self.0.div_euclid(NANOS_PER_SEC)
    }

    /// Snap down to a multiple of `interval_nanos`.
    ///
    /// This is how time bars find their opening boundary: every timestamp in
    /// the same bucket floors to the same value. Uses Euclidean division so
    /// pre-epoch timestamps bucket consistently.
    #[inline]
    pub fn floor_to(self, interval_nanos: i64) -> Self {
        debug_assert!(interval_nanos > 0, "bar interval must be positive");
        Self(self.0 - self.0.rem_euclid(interval_nanos))
    }

    /// Nanoseconds elapsed since `earlier` (negative if `earlier` is later).
    #[inline]
    pub const fn since(self, earlier: Ts) -> i64 {
        self.0 - earlier.0
    }
}

impl Add<i64> for Ts {
    type Output = Ts;
    #[inline]
    fn add(self, nanos: i64) -> Ts {
        Ts(self.0 + nanos)
    }
}

impl Sub<i64> for Ts {
    type Output = Ts;
    #[inline]
    fn sub(self, nanos: i64) -> Ts {
        Ts(self.0 - nanos)
    }
}

impl Sub for Ts {
    type Output = i64;
    #[inline]
    fn sub(self, other: Ts) -> i64 {
        self.0 - other.0
    }
}

impl fmt::Display for Ts {
    /// Renders as `<seconds>.<nanos>` — deliberately not a calendar format, so
    /// this crate stays free of a date-time dependency.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.secs();
        let nanos = self.0.rem_euclid(NANOS_PER_SEC);
        write!(f, "{secs}.{nanos:09}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_units() {
        let t = Ts::from_millis(1_700_000_000_123);
        assert_eq!(t.millis(), 1_700_000_000_123);
        assert_eq!(t.secs(), 1_700_000_000);
        assert_eq!(t.nanos(), 1_700_000_000_123_000_000);
    }

    #[test]
    fn floors_into_stable_buckets() {
        let minute = 60 * NANOS_PER_SEC;
        // 1_700_000_040_000 ms is exactly on a minute boundary; both of these
        // sit inside the minute that starts there.
        let a = Ts::from_millis(1_700_000_077_500);
        let b = Ts::from_millis(1_700_000_099_999);
        assert_eq!(a.floor_to(minute), b.floor_to(minute));
        assert_eq!(a.floor_to(minute), Ts::from_millis(1_700_000_040_000));
        // The next millisecond starts a new bucket.
        let c = Ts::from_millis(1_700_000_100_000);
        assert_ne!(b.floor_to(minute), c.floor_to(minute));
        assert_eq!(c.floor_to(minute), c);
    }

    #[test]
    fn floor_is_idempotent() {
        let sec = NANOS_PER_SEC;
        let t = Ts::from_millis(1_700_000_037_500).floor_to(sec);
        assert_eq!(t.floor_to(sec), t);
    }

    #[test]
    fn arithmetic_and_differences() {
        let t = Ts::from_secs(100);
        assert_eq!((t + NANOS_PER_SEC).secs(), 101);
        assert_eq!((t - NANOS_PER_SEC).secs(), 99);
        assert_eq!(Ts::from_secs(105) - t, 5 * NANOS_PER_SEC);
        assert_eq!(Ts::from_secs(105).since(t), 5 * NANOS_PER_SEC);
    }

    #[test]
    fn orders_chronologically() {
        let mut v = [Ts::from_secs(3), Ts::from_secs(1), Ts::from_secs(2)];
        v.sort();
        assert_eq!(v, [Ts::from_secs(1), Ts::from_secs(2), Ts::from_secs(3)]);
    }

    #[test]
    fn out_of_range_inputs_saturate_instead_of_wrapping() {
        // A venue sending a garbage timestamp must not panic or, in release,
        // wrap into a plausible-looking date.
        assert_eq!(Ts::from_millis(i64::MAX), Ts::MAX);
        assert_eq!(Ts::from_millis(i64::MIN), Ts::MIN);
        assert_eq!(Ts::from_secs(i64::MAX), Ts::MAX);
        // Ordering still holds at the boundary.
        assert!(Ts::from_millis(1_700_000_000_000) < Ts::MAX);
    }

    #[test]
    fn displays_seconds_and_nanos() {
        assert_eq!(Ts::from_millis(1_500).to_string(), "1.500000000");
    }
}
