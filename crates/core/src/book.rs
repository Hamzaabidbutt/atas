//! Level-2 order book maintenance.
//!
//! The book is kept as two ordered maps keyed by exact fixed-point price, so
//! a level updated a thousand times stays one level. Venue-specific gap rules
//! (Binance's `U`/`u` ranges, Bybit's sequence semantics) belong in the feed
//! adapters; what this type enforces is the venue-independent invariant that
//! updates must not go backwards.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fixed::{Price, Qty};
use crate::time::Ts;

/// Which side of the book a level sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BookSide {
    /// Resting buy orders. Best is the highest price.
    Bid,
    /// Resting sell orders. Best is the lowest price.
    Ask,
}

impl BookSide {
    /// The opposite side.
    pub const fn opposite(self) -> BookSide {
        match self {
            BookSide::Bid => BookSide::Ask,
            BookSide::Ask => BookSide::Bid,
        }
    }
}

/// A price level with its resting size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookLevel {
    /// Level price.
    pub price: Price,
    /// Total resting quantity at that price.
    pub qty: Qty,
}

/// Why a book update was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BookError {
    /// An update arrived with a sequence at or below the one already applied.
    /// Usually a duplicate from REST/websocket overlap — safe to drop.
    #[error("stale update: sequence {received} is not newer than {current}")]
    StaleSequence {
        /// Sequence carried by the rejected update.
        received: u64,
        /// Sequence already applied.
        current: u64,
    },
    /// A delta arrived before any snapshot. The caller must snapshot first.
    #[error("delta applied before an initial snapshot")]
    NotInitialised,
}

/// A level-2 order book for one instrument.
#[derive(Debug, Clone, Default)]
pub struct OrderBook {
    bids: BTreeMap<Price, Qty>,
    asks: BTreeMap<Price, Qty>,
    ts: Ts,
    sequence: u64,
    initialised: bool,
}

impl OrderBook {
    /// An empty, uninitialised book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entire book. This is the only way to initialise it, and the
    /// correct response to any detected gap.
    pub fn apply_snapshot(
        &mut self,
        ts: Ts,
        bids: impl IntoIterator<Item = (Price, Qty)>,
        asks: impl IntoIterator<Item = (Price, Qty)>,
        sequence: u64,
    ) {
        self.bids.clear();
        self.asks.clear();
        for (price, qty) in bids {
            if qty.is_positive() {
                self.bids.insert(price, qty);
            }
        }
        for (price, qty) in asks {
            if qty.is_positive() {
                self.asks.insert(price, qty);
            }
        }
        self.ts = ts;
        self.sequence = sequence;
        self.initialised = true;
    }

    /// Apply an incremental update. A quantity of zero removes the level.
    pub fn apply_delta(
        &mut self,
        ts: Ts,
        bids: impl IntoIterator<Item = (Price, Qty)>,
        asks: impl IntoIterator<Item = (Price, Qty)>,
        sequence: u64,
    ) -> Result<(), BookError> {
        if !self.initialised {
            return Err(BookError::NotInitialised);
        }
        // Sequence 0 means "venue supplies none"; skip the ordering check.
        if sequence != 0 && sequence <= self.sequence {
            return Err(BookError::StaleSequence {
                received: sequence,
                current: self.sequence,
            });
        }

        for (price, qty) in bids {
            update_level(&mut self.bids, price, qty);
        }
        for (price, qty) in asks {
            update_level(&mut self.asks, price, qty);
        }

        self.ts = ts;
        if sequence != 0 {
            self.sequence = sequence;
        }
        Ok(())
    }

    /// Whether a snapshot has been applied.
    #[inline]
    pub fn is_initialised(&self) -> bool {
        self.initialised
    }

    /// Timestamp of the most recently applied update.
    #[inline]
    pub fn ts(&self) -> Ts {
        self.ts
    }

    /// Sequence of the most recently applied update.
    #[inline]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Highest bid.
    pub fn best_bid(&self) -> Option<BookLevel> {
        self.bids
            .iter()
            .next_back()
            .map(|(&price, &qty)| BookLevel { price, qty })
    }

    /// Lowest ask.
    pub fn best_ask(&self) -> Option<BookLevel> {
        self.asks
            .iter()
            .next()
            .map(|(&price, &qty)| BookLevel { price, qty })
    }

    /// Ask minus bid. `None` until both sides have a level.
    pub fn spread(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(b), Some(a)) => Some(a.price - b.price),
            _ => None,
        }
    }

    /// Midpoint of the touch.
    pub fn mid(&self) -> Option<Price> {
        match (self.best_bid(), self.best_ask()) {
            (Some(b), Some(a)) => Some(Price::from_minor((b.price.minor() + a.price.minor()) / 2)),
            _ => None,
        }
    }

    /// A crossed or locked book means the feed is inconsistent — usually a
    /// dropped delta. Callers should re-snapshot rather than trust it.
    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(b), Some(a)) => b.price >= a.price,
            _ => false,
        }
    }

    /// The best `depth` bid levels, highest price first.
    pub fn bids(&self, depth: usize) -> Vec<BookLevel> {
        self.bids
            .iter()
            .rev()
            .take(depth)
            .map(|(&price, &qty)| BookLevel { price, qty })
            .collect()
    }

    /// The best `depth` ask levels, lowest price first.
    pub fn asks(&self, depth: usize) -> Vec<BookLevel> {
        self.asks
            .iter()
            .take(depth)
            .map(|(&price, &qty)| BookLevel { price, qty })
            .collect()
    }

    /// Resting size at an exact price, zero if the level is absent.
    pub fn qty_at(&self, side: BookSide, price: Price) -> Qty {
        let map = match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        };
        map.get(&price).copied().unwrap_or(Qty::ZERO)
    }

    /// Total resting size across the best `depth` levels of one side.
    pub fn depth_qty(&self, side: BookSide, depth: usize) -> Qty {
        match side {
            BookSide::Bid => self.bids.iter().rev().take(depth).map(|(_, &q)| q).sum(),
            BookSide::Ask => self.asks.iter().take(depth).map(|(_, &q)| q).sum(),
        }
    }

    /// Book imbalance over `depth` levels, as a ratio in `-1.0..=1.0`.
    ///
    /// `+1` means all resting size is on the bid, `-1` all on the ask. `None`
    /// when both sides are empty.
    pub fn imbalance(&self, depth: usize) -> Option<f64> {
        let bid = self.depth_qty(BookSide::Bid, depth).minor() as f64;
        let ask = self.depth_qty(BookSide::Ask, depth).minor() as f64;
        let total = bid + ask;
        if total <= 0.0 {
            None
        } else {
            Some((bid - ask) / total)
        }
    }

    /// Number of levels on each side, as `(bids, asks)`.
    pub fn level_count(&self) -> (usize, usize) {
        (self.bids.len(), self.asks.len())
    }

    /// Drop every level and mark the book uninitialised. Use on disconnect so
    /// stale depth is never shown as live.
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.initialised = false;
        self.sequence = 0;
    }
}

/// Insert, update or remove a level. Zero or negative quantity removes it.
fn update_level(map: &mut BTreeMap<Price, Qty>, price: Price, qty: Qty) {
    if qty.is_positive() {
        map.insert(price, qty);
    } else {
        map.remove(&price);
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

    fn seeded() -> OrderBook {
        let mut book = OrderBook::new();
        book.apply_snapshot(
            Ts::from_secs(1),
            [(px("99.98"), qty("5")), (px("99.99"), qty("3"))],
            [(px("100.01"), qty("4")), (px("100.02"), qty("7"))],
            10,
        );
        book
    }

    #[test]
    fn snapshot_establishes_the_touch() {
        let book = seeded();
        assert!(book.is_initialised());
        assert_eq!(book.best_bid().unwrap().price, px("99.99"));
        assert_eq!(book.best_ask().unwrap().price, px("100.01"));
        assert_eq!(book.spread().unwrap(), px("0.02"));
        assert_eq!(book.mid().unwrap(), px("100.00"));
        assert_eq!(book.level_count(), (2, 2));
    }

    #[test]
    fn snapshot_drops_zero_quantity_levels() {
        let mut book = OrderBook::new();
        book.apply_snapshot(
            Ts::from_secs(1),
            [(px("99.99"), qty("3")), (px("99.98"), qty("0"))],
            [(px("100.01"), qty("4"))],
            1,
        );
        assert_eq!(book.level_count(), (1, 1));
    }

    #[test]
    fn delta_updates_and_removes_levels() {
        let mut book = seeded();
        book.apply_delta(
            Ts::from_secs(2),
            [(px("99.99"), qty("8"))],          // resize
            [(px("100.01"), qty("0"))],         // remove
            11,
        )
        .unwrap();

        assert_eq!(book.qty_at(BookSide::Bid, px("99.99")), qty("8"));
        assert_eq!(book.best_ask().unwrap().price, px("100.02"));
        assert_eq!(book.qty_at(BookSide::Ask, px("100.01")), Qty::ZERO);
        assert_eq!(book.sequence(), 11);
        assert_eq!(book.ts(), Ts::from_secs(2));
    }

    #[test]
    fn delta_adds_new_levels() {
        let mut book = seeded();
        book.apply_delta(Ts::from_secs(2), [(px("99.97"), qty("2"))], [], 11)
            .unwrap();
        assert_eq!(book.level_count(), (3, 2));
        // A worse bid must not displace the best.
        assert_eq!(book.best_bid().unwrap().price, px("99.99"));
    }

    #[test]
    fn rejects_stale_and_uninitialised_updates() {
        let mut book = seeded();
        assert_eq!(
            book.apply_delta(Ts::from_secs(2), [], [], 10),
            Err(BookError::StaleSequence {
                received: 10,
                current: 10
            })
        );
        // A rejected update must leave the book untouched.
        assert_eq!(book.sequence(), 10);
        assert_eq!(book.ts(), Ts::from_secs(1));

        let mut fresh = OrderBook::new();
        assert_eq!(
            fresh.apply_delta(Ts::from_secs(1), [], [], 1),
            Err(BookError::NotInitialised)
        );
    }

    #[test]
    fn venues_without_sequences_skip_the_ordering_check() {
        let mut book = seeded();
        book.apply_delta(Ts::from_secs(2), [(px("99.99"), qty("9"))], [], 0)
            .unwrap();
        assert_eq!(book.qty_at(BookSide::Bid, px("99.99")), qty("9"));
        // Sequence is left as it was rather than reset to zero.
        assert_eq!(book.sequence(), 10);
    }

    #[test]
    fn detects_a_crossed_book() {
        let mut book = seeded();
        assert!(!book.is_crossed());
        book.apply_delta(Ts::from_secs(2), [(px("100.05"), qty("1"))], [], 11)
            .unwrap();
        assert!(book.is_crossed());
    }

    #[test]
    fn returns_depth_in_priority_order() {
        let book = seeded();
        let bids = book.bids(10);
        assert_eq!(bids[0].price, px("99.99"));
        assert_eq!(bids[1].price, px("99.98"));

        let asks = book.asks(10);
        assert_eq!(asks[0].price, px("100.01"));
        assert_eq!(asks[1].price, px("100.02"));

        // Depth caps the result.
        assert_eq!(book.bids(1).len(), 1);
    }

    #[test]
    fn aggregates_depth_quantity() {
        let book = seeded();
        assert_eq!(book.depth_qty(BookSide::Bid, 2), qty("8"));
        assert_eq!(book.depth_qty(BookSide::Ask, 2), qty("11"));
        assert_eq!(book.depth_qty(BookSide::Bid, 1), qty("3"));
    }

    #[test]
    fn imbalance_is_signed_and_bounded() {
        let book = seeded();
        // 8 bid vs 11 ask.
        let imb = book.imbalance(2).unwrap();
        assert!((imb - (8.0 - 11.0) / 19.0).abs() < 1e-12);
        assert!((-1.0..=1.0).contains(&imb));

        assert_eq!(OrderBook::new().imbalance(5), None);
    }

    #[test]
    fn clear_invalidates_the_book() {
        let mut book = seeded();
        book.clear();
        assert!(!book.is_initialised());
        assert_eq!(book.level_count(), (0, 0));
        assert_eq!(book.best_bid(), None);
        // And a delta is refused until a fresh snapshot arrives.
        assert_eq!(
            book.apply_delta(Ts::from_secs(3), [], [], 99),
            Err(BookError::NotInitialised)
        );
    }
}
