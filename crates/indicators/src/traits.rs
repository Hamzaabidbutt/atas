//! The indicator plugin surface.
//!
//! Indicators come in two shapes and the distinction is not cosmetic. Some
//! only make sense once a bar is complete (an average of closes); others must
//! see every print or they measure the wrong thing entirely (speed of tape,
//! large-print detection). Conflating them into one "update" hook is how
//! platforms end up with indicators that quietly recompute from stale bars.

use atas_core::Trade;
use atas_engine::Bar;

/// An indicator driven by completed bars.
pub trait BarIndicator {
    /// What the indicator produces.
    type Value;

    /// Human-readable name, for legends and workspace files.
    fn name(&self) -> &str;

    /// Fold in a bar and return the updated value.
    fn update(&mut self, bar: &Bar) -> Self::Value;

    /// The most recent value, or `None` before enough input has arrived.
    fn value(&self) -> Option<Self::Value>;

    /// Clear all state. Used at session boundaries and instrument changes.
    fn reset(&mut self);

    /// Whether enough input has arrived for the value to be meaningful.
    ///
    /// A 20-period average has a value after one bar, but it is not yet a
    /// 20-period average, and plotting it as one is a lie.
    fn is_warm(&self) -> bool {
        self.value().is_some()
    }
}

/// An indicator driven by individual trades.
pub trait TradeIndicator {
    /// What the indicator produces.
    type Value;

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Fold in a trade.
    fn update(&mut self, trade: &Trade) -> Self::Value;

    /// The most recent value.
    fn value(&self) -> Option<Self::Value>;

    /// Clear all state.
    fn reset(&mut self);
}
