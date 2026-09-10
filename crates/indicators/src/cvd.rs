//! Cumulative volume delta.
//!
//! CVD is the running sum of signed aggressive volume. On its own it is a
//! line; its value is in divergence — price making a new high while CVD does
//! not means the move up was not paid for by aggressive buying, and is the
//! single most-used order-flow read there is. [`Cvd`] therefore tracks the
//! swing extremes needed to detect that, rather than only the running total.

use atas_core::{Price, Qty};
use atas_engine::Bar;
use serde::{Deserialize, Serialize};

use crate::traits::BarIndicator;

/// A divergence between price and cumulative delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Divergence {
    /// Price made a higher high, CVD did not: the rally was not bought.
    Bearish,
    /// Price made a lower low, CVD did not: the decline was not sold.
    Bullish,
}

/// Cumulative volume delta with divergence detection.
#[derive(Debug, Clone, Default)]
pub struct Cvd {
    total: Qty,
    bars: u64,
    /// Highest price seen and the CVD at that moment.
    high_watermark: Option<(Price, Qty)>,
    /// Lowest price seen and the CVD at that moment.
    low_watermark: Option<(Price, Qty)>,
    last_divergence: Option<Divergence>,
}

impl Cvd {
    /// A fresh CVD at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The running total.
    #[inline]
    pub fn total(&self) -> Qty {
        self.total
    }

    /// How many bars have been folded in.
    #[inline]
    pub fn bars(&self) -> u64 {
        self.bars
    }

    /// The divergence detected on the most recent bar, if any.
    #[inline]
    pub fn divergence(&self) -> Option<Divergence> {
        self.last_divergence
    }
}

impl BarIndicator for Cvd {
    type Value = Qty;

    fn name(&self) -> &str {
        "CVD"
    }

    fn update(&mut self, bar: &Bar) -> Qty {
        self.total += bar.delta();
        self.bars += 1;
        self.last_divergence = None;

        match self.high_watermark {
            Some((price, cvd_at_high)) if bar.high > price => {
                // A higher high on lower cumulative delta: buyers were less
                // aggressive into this high than the last one.
                if self.total < cvd_at_high {
                    self.last_divergence = Some(Divergence::Bearish);
                }
                self.high_watermark = Some((bar.high, self.total));
            }
            None => self.high_watermark = Some((bar.high, self.total)),
            _ => {}
        }

        match self.low_watermark {
            Some((price, cvd_at_low)) if bar.low < price => {
                if self.total > cvd_at_low {
                    self.last_divergence = Some(Divergence::Bullish);
                }
                self.low_watermark = Some((bar.low, self.total));
            }
            None => self.low_watermark = Some((bar.low, self.total)),
            _ => {}
        }

        self.total
    }

    fn value(&self) -> Option<Qty> {
        (self.bars > 0).then_some(self.total)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}
