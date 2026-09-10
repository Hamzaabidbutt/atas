//! Fixed-point price and quantity arithmetic.
//!
//! Floating point is not acceptable for price levels in an order-flow system:
//! cluster ladders key cells by exact price, and `0.1 + 0.2 != 0.3` turns one
//! price level into two. Every price and quantity here is an `i64` in minor
//! units, scaled by [`SCALE`] (10^8). That gives 8 decimal places — enough for
//! satoshi-level crypto prices — and a range of roughly ±9.2e10 whole units,
//! far beyond any traded price or size.

use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Number of decimal places carried by [`Price`] and [`Qty`].
pub const DECIMALS: u32 = 8;

/// Minor units per whole unit: `10^DECIMALS`.
pub const SCALE: i64 = 100_000_000;

/// Errors produced when parsing a decimal string into a fixed-point value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseFixedError {
    /// The input was empty or contained only a sign.
    #[error("empty input")]
    Empty,
    /// The input contained a character that is not a digit, sign or point.
    #[error("invalid character {0:?} in decimal literal")]
    InvalidChar(char),
    /// The input contained more than one decimal point.
    #[error("more than one decimal point")]
    MultipleDecimalPoints,
    /// The value does not fit in 64 bits of minor units.
    #[error("value out of range for a 64-bit fixed-point number")]
    OutOfRange,
}

/// Parse a plain decimal string (`"-1234.5678"`) into minor units.
///
/// Digits beyond [`DECIMALS`] are rounded half away from zero rather than
/// rejected: a live feed sending one extra digit must not kill the session.
fn parse_minor(s: &str) -> Result<i64, ParseFixedError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseFixedError::Empty);
    }

    let (negative, digits) = match s.as_bytes()[0] {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() {
        return Err(ParseFixedError::Empty);
    }

    let mut int_part: i128 = 0;
    let mut frac: i128 = 0;
    let mut frac_digits = 0u32;
    let mut seen_point = false;
    // First digit dropped past DECIMALS, used for the rounding decision.
    let mut round_digit: Option<u8> = None;

    for ch in digits.chars() {
        match ch {
            '.' => {
                if seen_point {
                    return Err(ParseFixedError::MultipleDecimalPoints);
                }
                seen_point = true;
            }
            '0'..='9' => {
                let d = (ch as u8) - b'0';
                if !seen_point {
                    int_part = int_part
                        .checked_mul(10)
                        .and_then(|v| v.checked_add(d as i128))
                        .ok_or(ParseFixedError::OutOfRange)?;
                } else if frac_digits < DECIMALS {
                    frac = frac * 10 + d as i128;
                    frac_digits += 1;
                } else if round_digit.is_none() {
                    round_digit = Some(d);
                }
            }
            '_' => {}
            other => return Err(ParseFixedError::InvalidChar(other)),
        }
    }

    // Left-align the fraction to exactly DECIMALS places.
    for _ in frac_digits..DECIMALS {
        frac *= 10;
    }

    let mut minor = int_part
        .checked_mul(SCALE as i128)
        .and_then(|v| v.checked_add(frac))
        .ok_or(ParseFixedError::OutOfRange)?;

    // Round half away from zero on the dropped tail. The sign is applied
    // below, so rounding up on the magnitude is correct for both signs.
    if round_digit.is_some_and(|d| d >= 5) {
        minor += 1;
    }

    if negative {
        minor = -minor;
    }
    i64::try_from(minor).map_err(|_| ParseFixedError::OutOfRange)
}

/// Render minor units as a plain decimal string with `decimals` places.
fn format_minor(minor: i64, decimals: u32) -> String {
    debug_assert!(decimals <= DECIMALS);
    let negative = minor < 0;
    let abs = (minor as i128).unsigned_abs();

    let whole = abs / SCALE as u128;
    let frac = abs % SCALE as u128;

    let mut out = String::new();
    if negative && (whole != 0 || frac != 0) {
        out.push('-');
    }
    out.push_str(&whole.to_string());

    if decimals > 0 {
        // Truncate the 8-digit fraction down to `decimals` places.
        let divisor = 10u128.pow(DECIMALS - decimals);
        let shown = frac / divisor;
        out.push('.');
        let digits = shown.to_string();
        for _ in digits.len()..decimals as usize {
            out.push('0');
        }
        out.push_str(&digits);
    }
    out
}

/// Generates a fixed-point newtype over `i64` minor units.
macro_rules! fixed_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl $name {
            /// Zero.
            pub const ZERO: Self = Self(0);
            /// The smallest representable positive increment (1e-8).
            pub const MIN_UNIT: Self = Self(1);
            /// Largest representable value.
            pub const MAX: Self = Self(i64::MAX);
            /// Smallest representable value.
            pub const MIN: Self = Self(i64::MIN);

            /// Build from whole units (`from_units(3)` is `3.0`).
            #[inline]
            pub const fn from_units(units: i64) -> Self {
                Self(units * SCALE)
            }

            /// Build directly from minor units.
            #[inline]
            pub const fn from_minor(minor: i64) -> Self {
                Self(minor)
            }

            /// The underlying minor-unit count.
            #[inline]
            pub const fn minor(self) -> i64 {
                self.0
            }

            /// Parse a plain decimal string, as exchanges send them.
            pub fn parse(s: &str) -> Result<Self, ParseFixedError> {
                parse_minor(s).map(Self)
            }

            /// Convert to `f64`. For display and charting only — never feed the
            /// result back into price arithmetic.
            #[inline]
            pub fn to_f64(self) -> f64 {
                self.0 as f64 / SCALE as f64
            }

            /// Build from `f64`, rounding half away from zero. Use only at the
            /// boundary with float-based inputs.
            pub fn from_f64(v: f64) -> Self {
                let scaled = v * SCALE as f64;
                Self(if scaled < 0.0 {
                    (scaled - 0.5) as i64
                } else {
                    (scaled + 0.5) as i64
                })
            }

            /// Whether the value is exactly zero.
            #[inline]
            pub const fn is_zero(self) -> bool {
                self.0 == 0
            }

            /// Whether the value is greater than zero.
            #[inline]
            pub const fn is_positive(self) -> bool {
                self.0 > 0
            }

            /// Whether the value is less than zero.
            #[inline]
            pub const fn is_negative(self) -> bool {
                self.0 < 0
            }

            /// Absolute value.
            #[inline]
            pub const fn abs(self) -> Self {
                Self(self.0.abs())
            }

            /// The smaller of the two values.
            #[inline]
            pub fn min(self, other: Self) -> Self {
                Self(self.0.min(other.0))
            }

            /// The larger of the two values.
            #[inline]
            pub fn max(self, other: Self) -> Self {
                Self(self.0.max(other.0))
            }

            /// Saturating add — used on hot aggregation paths where a panic is
            /// worse than a clamped total.
            #[inline]
            pub const fn saturating_add(self, other: Self) -> Self {
                Self(self.0.saturating_add(other.0))
            }

            /// Addition returning `None` on overflow.
            #[inline]
            pub const fn checked_add(self, other: Self) -> Option<Self> {
                match self.0.checked_add(other.0) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Subtraction returning `None` on overflow.
            #[inline]
            pub const fn checked_sub(self, other: Self) -> Option<Self> {
                match self.0.checked_sub(other.0) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Format with an explicit number of decimal places.
            pub fn format(self, decimals: u32) -> String {
                format_minor(self.0, decimals.min(DECIMALS))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Trim trailing zeros from the fraction only, keeping at least
                // one decimal place. Trimming the whole string would turn
                // "100.00000000" into "1".
                let s = format_minor(self.0, DECIMALS);
                let point = s.find('.').expect("format_minor emits a decimal point");
                let (int_part, frac) = s.split_at(point + 1);
                let kept = frac.trim_end_matches('0');
                if kept.is_empty() {
                    write!(f, "{int_part}0")
                } else {
                    write!(f, "{int_part}{kept}")
                }
            }
        }

        impl std::str::FromStr for $name {
            type Err = ParseFixedError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl Add for $name {
            type Output = Self;
            #[inline]
            fn add(self, rhs: Self) -> Self {
                Self(self.0 + rhs.0)
            }
        }

        impl Sub for $name {
            type Output = Self;
            #[inline]
            fn sub(self, rhs: Self) -> Self {
                Self(self.0 - rhs.0)
            }
        }

        impl Neg for $name {
            type Output = Self;
            #[inline]
            fn neg(self) -> Self {
                Self(-self.0)
            }
        }

        impl Mul<i64> for $name {
            type Output = Self;
            #[inline]
            fn mul(self, rhs: i64) -> Self {
                Self(self.0 * rhs)
            }
        }

        impl Div<i64> for $name {
            type Output = Self;
            #[inline]
            fn div(self, rhs: i64) -> Self {
                Self(self.0 / rhs)
            }
        }

        impl AddAssign for $name {
            #[inline]
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl SubAssign for $name {
            #[inline]
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        impl std::iter::Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                iter.fold(Self::ZERO, |a, b| a + b)
            }
        }
    };
}

fixed_type!(
    Price,
    "A price in fixed-point minor units (8 decimal places).\n\nOrdering is exact, so prices are safe as map keys and cluster-ladder indices."
);
fixed_type!(
    Qty,
    "A quantity (contracts, coins, shares) in fixed-point minor units (8 decimal places)."
);

/// Rounding direction when snapping a price to a tick boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// Toward negative infinity.
    Down,
    /// Toward positive infinity.
    Up,
    /// To the closest boundary, ties going up.
    Nearest,
}

impl Price {
    /// Snap this price to a multiple of `tick`.
    ///
    /// Panics if `tick` is not positive — a zero tick size is a configuration
    /// bug that must surface loudly, not silently produce garbage ladders.
    pub fn round_to_tick(self, tick: Price, mode: Rounding) -> Price {
        assert!(tick.0 > 0, "tick size must be positive, got {tick:?}");
        let t = tick.0;
        // Euclidean remainder so negative prices round consistently.
        let rem = self.0.rem_euclid(t);
        let floor = self.0 - rem;
        match mode {
            Rounding::Down => Price(floor),
            Rounding::Up => Price(if rem == 0 { floor } else { floor + t }),
            Rounding::Nearest => {
                if rem * 2 >= t {
                    Price(floor + t)
                } else {
                    Price(floor)
                }
            }
        }
    }

    /// Index of this price on a tick ladder anchored at zero.
    ///
    /// This is the cluster-ladder row key: two trades at the same tick always
    /// produce the same index, which is exactly the property floats lose.
    #[inline]
    pub fn tick_index(self, tick: Price) -> i64 {
        debug_assert!(tick.0 > 0, "tick size must be positive");
        self.0.div_euclid(tick.0)
    }

    /// Inverse of [`Price::tick_index`].
    #[inline]
    pub fn from_tick_index(index: i64, tick: Price) -> Price {
        Price(index * tick.0)
    }

    /// Notional value of `qty` at this price, as a [`Qty`]-scaled amount.
    ///
    /// Computed through `i128` so a large size at a large price cannot wrap.
    pub fn notional(self, qty: Qty) -> Qty {
        let product = (self.0 as i128) * (qty.0 as i128) / (SCALE as i128);
        Qty(product.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_decimals() {
        assert_eq!(Price::parse("1234.5678").unwrap().0, 123_456_780_000);
        assert_eq!(Price::parse("0.00000001").unwrap().0, 1);
        assert_eq!(Price::parse("-0.5").unwrap().0, -50_000_000);
        assert_eq!(Price::parse("42").unwrap().0, 42 * SCALE);
        assert_eq!(Price::parse("  7.25  ").unwrap().0, 725_000_000);
        assert_eq!(Price::parse("+3.5").unwrap().0, 350_000_000);
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(Price::parse(""), Err(ParseFixedError::Empty));
        assert_eq!(Price::parse("-"), Err(ParseFixedError::Empty));
        assert_eq!(
            Price::parse("1.2.3"),
            Err(ParseFixedError::MultipleDecimalPoints)
        );
        assert_eq!(Price::parse("12a"), Err(ParseFixedError::InvalidChar('a')));
    }

    #[test]
    fn rounds_excess_precision_rather_than_failing() {
        // A feed sending 9 decimals must not take the session down.
        assert_eq!(Price::parse("0.000000014").unwrap().0, 1);
        assert_eq!(Price::parse("0.000000016").unwrap().0, 2);
        assert_eq!(Price::parse("0.000000015").unwrap().0, 2);
    }

    #[test]
    fn exact_decimal_arithmetic() {
        // The canonical float failure: 0.1 + 0.2 == 0.3 must hold exactly.
        let a = Price::parse("0.1").unwrap();
        let b = Price::parse("0.2").unwrap();
        assert_eq!(a + b, Price::parse("0.3").unwrap());
    }

    #[test]
    fn round_trips_through_display() {
        for s in ["1234.5678", "0.00000001", "42", "-17.5"] {
            let p = Price::parse(s).unwrap();
            assert_eq!(Price::parse(&p.to_string()).unwrap(), p, "input {s}");
        }
        assert_eq!(Price::parse("42").unwrap().to_string(), "42.0");
        assert_eq!(Price::parse("1234.5678").unwrap().to_string(), "1234.5678");
    }

    #[test]
    fn display_keeps_zeros_in_the_integer_part() {
        // Regression: trimming the whole string turned "100" into "1".
        assert_eq!(Price::parse("100").unwrap().to_string(), "100.0");
        assert_eq!(Price::parse("1000.10").unwrap().to_string(), "1000.1");
        assert_eq!(Price::parse("0").unwrap().to_string(), "0.0");
        assert_eq!(Price::parse("-100").unwrap().to_string(), "-100.0");
        assert_eq!(Price::parse("20500").unwrap().to_string(), "20500.0");
    }

    #[test]
    fn formats_with_fixed_decimals() {
        let p = Price::parse("1234.5").unwrap();
        assert_eq!(p.format(2), "1234.50");
        assert_eq!(p.format(0), "1234");
        assert_eq!(Price::parse("-0.25").unwrap().format(2), "-0.25");
    }

    #[test]
    fn snaps_to_tick_boundaries() {
        let tick = Price::parse("0.25").unwrap();
        let p = Price::parse("5312.63").unwrap();
        assert_eq!(p.round_to_tick(tick, Rounding::Down).to_string(), "5312.5");
        assert_eq!(p.round_to_tick(tick, Rounding::Up).to_string(), "5312.75");
        assert_eq!(
            p.round_to_tick(tick, Rounding::Nearest).to_string(),
            "5312.75"
        );

        // Exact boundaries stay put in every mode.
        let exact = Price::parse("5312.75").unwrap();
        for mode in [Rounding::Down, Rounding::Up, Rounding::Nearest] {
            assert_eq!(exact.round_to_tick(tick, mode), exact, "{mode:?}");
        }
    }

    #[test]
    fn tick_index_is_stable_and_invertible() {
        let tick = Price::parse("0.25").unwrap();
        let a = Price::parse("5312.75").unwrap();
        let b = Price::parse("5312.75").unwrap();
        assert_eq!(a.tick_index(tick), b.tick_index(tick));

        let idx = a.tick_index(tick);
        assert_eq!(Price::from_tick_index(idx, tick), a);

        // Adjacent ticks differ by exactly one index.
        let next = a + tick;
        assert_eq!(next.tick_index(tick), idx + 1);
    }

    #[test]
    fn notional_does_not_overflow_at_scale() {
        let price = Price::parse("95000").unwrap();
        let qty = Qty::parse("12.5").unwrap();
        assert_eq!(price.notional(qty), Qty::parse("1187500").unwrap());
    }

    #[test]
    fn f64_conversion_round_trips_within_precision() {
        let p = Price::parse("5312.75").unwrap();
        assert_eq!(Price::from_f64(p.to_f64()), p);
    }
}
