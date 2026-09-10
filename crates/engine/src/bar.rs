//! Bars: OHLC plus the order-flow payload that makes them useful.

use atas_core::{Price, Qty, Side, Trade, Ts};
use serde::{Deserialize, Serialize};

use crate::cluster::{ClusterLadder, ValueArea};

/// A single bar with its cluster ladder.
///
/// The OHLC fields exist so a bar can be drawn as a candle, but the ladder is
/// the point: it is what separates this from every other charting package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    /// Timestamp of the first trade in the bar. For time bars this is the
    /// bucket boundary, not the first trade's own timestamp.
    pub open_ts: Ts,
    /// Timestamp of the last trade recorded so far.
    pub close_ts: Ts,
    /// First traded price.
    pub open: Price,
    /// Highest traded price.
    pub high: Price,
    /// Lowest traded price.
    pub low: Price,
    /// Last traded price.
    pub close: Price,
    /// Total traded volume.
    pub volume: Qty,
    /// Number of trades.
    pub trades: u32,
    /// Per-price volume breakdown.
    pub clusters: ClusterLadder,
    /// Whether the bar is finished. A forming bar is still mutating.
    pub closed: bool,
}

impl Bar {
    /// Open a new bar on its first trade.
    pub fn open(open_ts: Ts, trade: &Trade, tick_size: Price) -> Self {
        let mut bar = Self {
            open_ts,
            close_ts: trade.ts,
            open: trade.price,
            high: trade.price,
            low: trade.price,
            close: trade.price,
            volume: Qty::ZERO,
            trades: 0,
            clusters: ClusterLadder::new(tick_size),
            closed: false,
        };
        bar.apply(trade);
        bar
    }

    /// Fold a trade into this bar.
    pub fn apply(&mut self, trade: &Trade) {
        self.high = self.high.max(trade.price);
        self.low = self.low.min(trade.price);
        self.close = trade.price;
        self.close_ts = trade.ts;
        self.volume += trade.qty;
        self.trades += 1;
        self.clusters.add(trade.price, trade.qty, trade.aggressor);
    }

    /// Ask volume minus bid volume.
    #[inline]
    pub fn delta(&self) -> Qty {
        self.clusters.delta()
    }

    /// Whether the bar closed at or above its open.
    #[inline]
    pub fn is_up(&self) -> bool {
        self.close >= self.open
    }

    /// High minus low.
    #[inline]
    pub fn range(&self) -> Price {
        self.high - self.low
    }

    /// The most-traded price within the bar.
    #[inline]
    pub fn poc(&self) -> Option<Price> {
        self.clusters.poc()
    }

    /// The bar's value area.
    #[inline]
    pub fn value_area(&self, fraction: f64) -> Option<ValueArea> {
        self.clusters.value_area(fraction)
    }

    /// Signed volume of one side.
    pub fn side_volume(&self, side: Side) -> Qty {
        match side {
            Side::Buy => self.clusters.total_ask(),
            Side::Sell => self.clusters.total_bid(),
        }
    }

    /// Mark the bar finished.
    #[inline]
    pub fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(s: &str) -> Price {
        Price::parse(s).unwrap()
    }
    fn qty(s: &str) -> Qty {
        Qty::parse(s).unwrap()
    }
    fn trade(ms: i64, price: &str, q: &str, side: Side) -> Trade {
        Trade::new(Ts::from_millis(ms), px(price), qty(q), side, 0)
    }

    #[test]
    fn opening_records_the_first_trade() {
        let t = trade(1_000, "100.00", "5", Side::Buy);
        let bar = Bar::open(Ts::from_millis(1_000), &t, px("0.25"));

        assert_eq!(bar.open, px("100.00"));
        assert_eq!(bar.high, px("100.00"));
        assert_eq!(bar.low, px("100.00"));
        assert_eq!(bar.close, px("100.00"));
        assert_eq!(bar.volume, qty("5"));
        assert_eq!(bar.trades, 1);
        assert_eq!(bar.delta(), qty("5"));
        assert!(!bar.closed);
    }

    #[test]
    fn tracks_ohlc_across_trades() {
        let mut bar = Bar::open(
            Ts::from_millis(0),
            &trade(0, "100.00", "1", Side::Buy),
            px("0.25"),
        );
        bar.apply(&trade(1, "101.00", "1", Side::Buy));
        bar.apply(&trade(2, "99.00", "1", Side::Sell));
        bar.apply(&trade(3, "100.50", "1", Side::Buy));

        assert_eq!(bar.open, px("100.00"));
        assert_eq!(bar.high, px("101.00"));
        assert_eq!(bar.low, px("99.00"));
        assert_eq!(bar.close, px("100.50"));
        assert_eq!(bar.range(), px("2.00"));
        assert_eq!(bar.volume, qty("4"));
        assert_eq!(bar.trades, 4);
        assert_eq!(bar.close_ts, Ts::from_millis(3));
        assert!(bar.is_up());
    }

    #[test]
    fn delta_and_side_volume_agree_with_the_ladder() {
        let mut bar = Bar::open(
            Ts::from_millis(0),
            &trade(0, "100.00", "7", Side::Buy),
            px("0.25"),
        );
        bar.apply(&trade(1, "100.00", "3", Side::Sell));

        assert_eq!(bar.side_volume(Side::Buy), qty("7"));
        assert_eq!(bar.side_volume(Side::Sell), qty("3"));
        assert_eq!(bar.delta(), qty("4"));
        assert_eq!(bar.volume, bar.clusters.total_volume());
    }

    #[test]
    fn down_bar_is_not_up() {
        let mut bar = Bar::open(
            Ts::from_millis(0),
            &trade(0, "100.00", "1", Side::Buy),
            px("0.25"),
        );
        bar.apply(&trade(1, "99.00", "1", Side::Sell));
        assert!(!bar.is_up());
    }

    #[test]
    fn poc_comes_from_the_ladder() {
        let mut bar = Bar::open(
            Ts::from_millis(0),
            &trade(0, "100.00", "1", Side::Buy),
            px("0.25"),
        );
        bar.apply(&trade(1, "100.25", "50", Side::Buy));
        assert_eq!(bar.poc().unwrap(), px("100.25"));
    }
}
