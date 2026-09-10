//! Volume-weighted average price with standard-deviation bands.
//!
//! VWAP is an execution benchmark before it is a signal: it is the price the
//! average participant actually paid, so institutions measure their fills
//! against it. Anchoring matters — session VWAP and VWAP anchored to a
//! specific event answer different questions — so the anchor is the caller's
//! to choose via [`Vwap::reset`].

use atas_core::{Price, Qty, SCALE};
use atas_engine::Bar;
use serde::{Deserialize, Serialize};

use crate::traits::BarIndicator;

/// VWAP and its deviation bands at one point in time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VwapValue {
    /// The volume-weighted average price.
    pub vwap: Price,
    /// One standard deviation, in price terms.
    pub deviation: Price,
    /// Total volume behind the average.
    pub volume: Qty,
}

impl VwapValue {
    /// The band `multiple` standard deviations above VWAP.
    pub fn upper(&self, multiple: f64) -> Price {
        Price::from_minor(self.vwap.minor() + (self.deviation.minor() as f64 * multiple) as i64)
    }

    /// The band `multiple` standard deviations below VWAP.
    pub fn lower(&self, multiple: f64) -> Price {
        Price::from_minor(self.vwap.minor() - (self.deviation.minor() as f64 * multiple) as i64)
    }
}

/// Volume-weighted average price over everything since the last reset.
#[derive(Debug, Clone, Default)]
pub struct Vwap {
    /// Σ(price × volume), in `i128` because a session's notional overflows
    /// `i64` minor units long before the day is out.
    weighted_sum: i128,
    /// Σ(price² × volume), for the variance.
    weighted_square_sum: i128,
    volume: i128,
    bars: u64,
}

impl Vwap {
    /// A fresh VWAP anchored at the next bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold in a raw price and volume, for anchoring to trades rather than
    /// bars.
    pub fn add(&mut self, price: Price, qty: Qty) {
        let p = price.minor() as i128;
        let q = qty.minor() as i128;
        self.weighted_sum += p * q;
        // Scaled down once here so the square does not overflow on a busy day.
        self.weighted_square_sum += (p * p / SCALE as i128) * q;
        self.volume += q;
    }

    /// Total volume behind the current average.
    pub fn volume(&self) -> Qty {
        Qty::from_minor(self.volume.clamp(0, i64::MAX as i128) as i64)
    }

    fn compute(&self) -> Option<VwapValue> {
        if self.volume <= 0 {
            return None;
        }
        let vwap = self.weighted_sum / self.volume;

        // Var(P) = E[P²] − E[P]², weighted by volume. Clamped at zero because
        // the scaling above can leave a tiny negative from rounding when every
        // trade was at the same price.
        let mean_square = self.weighted_square_sum / self.volume;
        let square_mean = vwap * vwap / SCALE as i128;
        let variance = (mean_square - square_mean).max(0);

        // The standard deviation is a display statistic on top of an exact
        // VWAP, so taking the square root in floating point is safe here —
        // unlike the average itself, it never becomes a ladder key.
        let deviation = (variance as f64 * SCALE as f64).sqrt() as i64;

        Some(VwapValue {
            vwap: Price::from_minor(vwap as i64),
            deviation: Price::from_minor(deviation),
            volume: self.volume(),
        })
    }
}

impl BarIndicator for Vwap {
    type Value = Option<VwapValue>;

    fn name(&self) -> &str {
        "VWAP"
    }

    fn update(&mut self, bar: &Bar) -> Option<VwapValue> {
        // Weight by each cluster level rather than by the bar's typical price:
        // the ladder knows exactly how much traded at every price, so there is
        // no reason to approximate with (H+L+C)/3.
        for row in bar.clusters.rows() {
            let total = row.cluster.total();
            if total.is_positive() {
                self.add(row.price, total);
            }
        }
        self.bars += 1;
        self.compute()
    }

    /// Gated on volume, not on bar count: [`Vwap::add`] exists so a VWAP can
    /// be anchored to a trade rather than a bar boundary, and gating on bars
    /// made a trade-anchored VWAP report no value while holding real data.
    fn value(&self) -> Option<Option<VwapValue>> {
        Some(self.compute())
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn is_warm(&self) -> bool {
        self.volume > 0
    }
}
