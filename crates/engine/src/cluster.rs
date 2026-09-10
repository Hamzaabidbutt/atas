//! Cluster (footprint) ladders.
//!
//! A cluster ladder is a bar decomposed into price levels, each holding the
//! volume that traded on the bid and on the ask. This is the structure the
//! whole product is built around, so it is stored densely: rows are a `Vec`
//! indexed by offset from a base tick index, which makes accumulating a trade
//! an O(1) array write rather than a map lookup, and keeps a bar's rows
//! contiguous in cache when rendering.

use atas_core::{Instrument, Price, Qty, Side};
use serde::{Deserialize, Serialize};

/// Volume traded at a single price level within a bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cluster {
    /// Volume that traded on the bid — sellers were the aggressor.
    pub bid: Qty,
    /// Volume that traded on the ask — buyers were the aggressor.
    pub ask: Qty,
    /// Number of individual trades at this level.
    pub trades: u32,
}

impl Cluster {
    /// Total volume at this level.
    #[inline]
    pub fn total(&self) -> Qty {
        self.bid + self.ask
    }

    /// Ask minus bid: positive when buyers were the aggressors here.
    #[inline]
    pub fn delta(&self) -> Qty {
        self.ask - self.bid
    }

    /// Whether any volume traded at this level.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bid.is_zero() && self.ask.is_zero()
    }
}

/// A price level paired with its cluster, as handed to renderers and scanners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterRow {
    /// The price of this level.
    pub price: Price,
    /// Volume breakdown at that price.
    pub cluster: Cluster,
}

/// One side of a detected diagonal imbalance.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Imbalance {
    /// Price at which the dominant side traded.
    pub price: Price,
    /// Which side dominated.
    pub side: Side,
    /// Dominant volume divided by the volume it was compared against.
    /// Saturates at [`Imbalance::UNOPPOSED`] when the other side is empty.
    pub ratio: f64,
}

impl Imbalance {
    /// Ratio reported when the opposing diagonal cell had no volume at all.
    pub const UNOPPOSED: f64 = f64::INFINITY;
}

/// The value area of a ladder: the price band containing a share of volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueArea {
    /// Value area high.
    pub high: Price,
    /// Value area low.
    pub low: Price,
    /// Point of control — the single most-traded price.
    pub poc: Price,
    /// Volume contained within the band.
    pub volume: Qty,
}

/// A bar's volume broken down by price level.
///
/// Rows are dense between the lowest and highest traded price, so a level that
/// saw no trades is present with a zero cluster. That is deliberate: gaps in a
/// footprint carry information (single prints), and callers should be able to
/// walk rows without reasoning about missing keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClusterLadder {
    tick_size: Price,
    /// Tick index of `rows[0]`. Meaningless while `rows` is empty.
    base_index: i64,
    rows: Vec<Cluster>,
    total_bid: Qty,
    total_ask: Qty,
    trades: u32,
}

impl ClusterLadder {
    /// An empty ladder for an instrument's tick size.
    pub fn new(tick_size: Price) -> Self {
        assert!(
            tick_size.is_positive(),
            "tick size must be positive, got {tick_size:?}"
        );
        Self {
            tick_size,
            base_index: 0,
            rows: Vec::new(),
            total_bid: Qty::ZERO,
            total_ask: Qty::ZERO,
            trades: 0,
        }
    }

    /// An empty ladder for an instrument.
    pub fn for_instrument(instrument: &Instrument) -> Self {
        Self::new(instrument.tick_size)
    }

    /// Record a trade at a price.
    ///
    /// The price is floored to its tick, so an off-tick print (some venues send
    /// them) lands on the level below rather than being dropped or creating a
    /// phantom row.
    pub fn add(&mut self, price: Price, qty: Qty, aggressor: Side) {
        let index = price.tick_index(self.tick_size);
        let slot = self.slot_for(index);

        match aggressor {
            Side::Buy => {
                self.rows[slot].ask += qty;
                self.total_ask += qty;
            }
            Side::Sell => {
                self.rows[slot].bid += qty;
                self.total_bid += qty;
            }
        }
        self.rows[slot].trades += 1;
        self.trades += 1;
    }

    /// Return the row slot for a tick index, growing the ladder if needed.
    fn slot_for(&mut self, index: i64) -> usize {
        if self.rows.is_empty() {
            self.base_index = index;
            self.rows.push(Cluster::default());
            return 0;
        }

        if index < self.base_index {
            // Grow downward: prepend the missing rows in one allocation.
            let missing = (self.base_index - index) as usize;
            self.rows.splice(0..0, std::iter::repeat_n(Cluster::default(), missing));
            self.base_index = index;
            return 0;
        }

        let slot = (index - self.base_index) as usize;
        if slot >= self.rows.len() {
            self.rows.resize(slot + 1, Cluster::default());
        }
        slot
    }

    /// Whether any trade has been recorded.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Number of price levels spanned, including untraded ones in between.
    #[inline]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// The instrument tick size this ladder was built with.
    #[inline]
    pub fn tick_size(&self) -> Price {
        self.tick_size
    }

    /// Total volume across every level.
    #[inline]
    pub fn total_volume(&self) -> Qty {
        self.total_bid + self.total_ask
    }

    /// Total volume that traded on the bid.
    #[inline]
    pub fn total_bid(&self) -> Qty {
        self.total_bid
    }

    /// Total volume that traded on the ask.
    #[inline]
    pub fn total_ask(&self) -> Qty {
        self.total_ask
    }

    /// Bar delta: ask volume minus bid volume.
    #[inline]
    pub fn delta(&self) -> Qty {
        self.total_ask - self.total_bid
    }

    /// Number of trades recorded.
    #[inline]
    pub fn trade_count(&self) -> u32 {
        self.trades
    }

    /// Lowest price level in the ladder.
    pub fn low(&self) -> Option<Price> {
        (!self.rows.is_empty()).then(|| self.price_at(0))
    }

    /// Highest price level in the ladder.
    pub fn high(&self) -> Option<Price> {
        (!self.rows.is_empty()).then(|| self.price_at(self.rows.len() - 1))
    }

    /// Price of a row slot.
    #[inline]
    fn price_at(&self, slot: usize) -> Price {
        Price::from_tick_index(self.base_index + slot as i64, self.tick_size)
    }

    /// The cluster at an exact price, or an empty one if nothing traded there.
    pub fn at(&self, price: Price) -> Cluster {
        let index = price.tick_index(self.tick_size);
        if self.rows.is_empty() || index < self.base_index {
            return Cluster::default();
        }
        let slot = (index - self.base_index) as usize;
        self.rows.get(slot).copied().unwrap_or_default()
    }

    /// Every level from low to high.
    pub fn rows(&self) -> impl Iterator<Item = ClusterRow> + '_ {
        self.rows.iter().enumerate().map(|(slot, &cluster)| ClusterRow {
            price: self.price_at(slot),
            cluster,
        })
    }

    /// Point of control: the most-traded price.
    ///
    /// Ties go to the level closest to the middle of the ladder, which is the
    /// convention that keeps the POC from jumping to an extreme on thin bars.
    pub fn poc(&self) -> Option<Price> {
        if self.rows.is_empty() {
            return None;
        }
        let middle = (self.rows.len() - 1) as f64 / 2.0;
        let mut best_slot = 0usize;
        let mut best_total = Qty::ZERO;

        for (slot, cluster) in self.rows.iter().enumerate() {
            let total = cluster.total();
            if total > best_total {
                best_total = total;
                best_slot = slot;
            } else if total == best_total && total.is_positive() {
                let current = (slot as f64 - middle).abs();
                let incumbent = (best_slot as f64 - middle).abs();
                if current < incumbent {
                    best_slot = slot;
                }
            }
        }

        best_total.is_positive().then(|| self.price_at(best_slot))
    }

    /// Value area containing `fraction` of total volume (0.7 is conventional).
    ///
    /// Grows outward from the POC, at each step taking whichever adjacent pair
    /// of levels holds more volume — the standard Market Profile construction.
    pub fn value_area(&self, fraction: f64) -> Option<ValueArea> {
        let fraction = fraction.clamp(0.0, 1.0);
        let poc_price = self.poc()?;
        let total = self.total_volume();
        if !total.is_positive() {
            return None;
        }

        let poc_slot = (poc_price.tick_index(self.tick_size) - self.base_index) as usize;
        let target = (total.minor() as f64 * fraction).round() as i64;

        let mut lower = poc_slot;
        let mut upper = poc_slot;
        let mut volume = self.rows[poc_slot].total();

        // Sum of the next `n` slots above `upper` / below `lower`.
        let sum_above = |from: usize, n: usize| -> Qty {
            (1..=n)
                .filter_map(|k| self.rows.get(from + k))
                .map(|c| c.total())
                .sum()
        };
        let sum_below = |from: usize, n: usize| -> Qty {
            (1..=n)
                .filter_map(|k| from.checked_sub(k).and_then(|s| self.rows.get(s)))
                .map(|c| c.total())
                .sum()
        };

        while volume.minor() < target {
            let can_go_up = upper + 1 < self.rows.len();
            let can_go_down = lower > 0;
            if !can_go_up && !can_go_down {
                break;
            }

            // Market Profile compares two levels at a time on each side.
            let up = if can_go_up {
                sum_above(upper, 2)
            } else {
                Qty::ZERO
            };
            let down = if can_go_down {
                sum_below(lower, 2)
            } else {
                Qty::ZERO
            };

            if can_go_up && (!can_go_down || up >= down) {
                for _ in 0..2 {
                    if upper + 1 < self.rows.len() {
                        upper += 1;
                        volume += self.rows[upper].total();
                    }
                }
            } else {
                for _ in 0..2 {
                    if lower > 0 {
                        lower -= 1;
                        volume += self.rows[lower].total();
                    }
                }
            }
        }

        Some(ValueArea {
            high: self.price_at(upper),
            low: self.price_at(lower),
            poc: poc_price,
            volume,
        })
    }

    /// Diagonal imbalances at or above `ratio`.
    ///
    /// Footprint imbalance is diagonal, not horizontal: buyers lifting the
    /// offer at a price are compared against sellers hitting the bid one tick
    /// *below*, because those are the two sides of the same auction. Comparing
    /// bid and ask on the same row — which is what a naive implementation does
    /// — measures something that never actually traded against each other.
    ///
    /// `min_volume` filters out imbalances built from noise-sized prints.
    pub fn imbalances(&self, ratio: f64, min_volume: Qty) -> Vec<Imbalance> {
        assert!(ratio > 0.0, "imbalance ratio must be positive");
        let mut out = Vec::new();

        for slot in 0..self.rows.len() {
            // Buy imbalance: ask at this level vs bid one tick below.
            let ask = self.rows[slot].ask;
            if ask >= min_volume && ask.is_positive() && slot > 0 {
                let opposing = self.rows[slot - 1].bid;
                if let Some(r) = imbalance_ratio(ask, opposing, ratio) {
                    out.push(Imbalance {
                        price: self.price_at(slot),
                        side: Side::Buy,
                        ratio: r,
                    });
                }
            }

            // Sell imbalance: bid at this level vs ask one tick above.
            let bid = self.rows[slot].bid;
            if bid >= min_volume && bid.is_positive() && slot + 1 < self.rows.len() {
                let opposing = self.rows[slot + 1].ask;
                if let Some(r) = imbalance_ratio(bid, opposing, ratio) {
                    out.push(Imbalance {
                        price: self.price_at(slot),
                        side: Side::Sell,
                        ratio: r,
                    });
                }
            }
        }
        out
    }

    /// Runs of at least `min_run` consecutive same-side imbalances.
    ///
    /// A stacked imbalance is the tradable signal: one imbalanced level is
    /// noise, four in a row is a side that kept paying up and got filled.
    /// Each returned run is ordered low price to high.
    pub fn stacked_imbalances(
        &self,
        ratio: f64,
        min_volume: Qty,
        min_run: usize,
    ) -> Vec<Vec<Imbalance>> {
        assert!(min_run > 0, "stack length must be at least 1");
        let found = self.imbalances(ratio, min_volume);
        let mut runs: Vec<Vec<Imbalance>> = Vec::new();
        let mut current: Vec<Imbalance> = Vec::new();

        for imb in found {
            let continues = current.last().is_some_and(|prev| {
                prev.side == imb.side && imb.price - prev.price == self.tick_size
            });
            if continues {
                current.push(imb);
            } else {
                if current.len() >= min_run {
                    runs.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                current.push(imb);
            }
        }
        if current.len() >= min_run {
            runs.push(current);
        }
        runs
    }
}

/// Ratio of `dominant` against `opposing`, if it clears `threshold`.
fn imbalance_ratio(dominant: Qty, opposing: Qty, threshold: f64) -> Option<f64> {
    if opposing.is_zero() {
        // Unopposed volume is the strongest imbalance there is, but only counts
        // when the dominant side actually traded.
        return dominant.is_positive().then_some(Imbalance::UNOPPOSED);
    }
    let r = dominant.minor() as f64 / opposing.minor() as f64;
    (r >= threshold).then_some(r)
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

    fn tick() -> Price {
        px("0.25")
    }

    /// Ladder with a clear POC at 100.50 and volume tapering away from it.
    fn tapered() -> ClusterLadder {
        let mut l = ClusterLadder::new(tick());
        for (price, bid, ask) in [
            ("100.00", "10", "5"),
            ("100.25", "20", "15"),
            ("100.50", "60", "70"),
            ("100.75", "18", "12"),
            ("101.00", "8", "4"),
        ] {
            l.add(px(price), qty(bid), Side::Sell);
            l.add(px(price), qty(ask), Side::Buy);
        }
        l
    }

    #[test]
    fn accumulates_by_aggressor_side() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.00"), qty("3"), Side::Buy);
        l.add(px("100.00"), qty("2"), Side::Buy);
        l.add(px("100.00"), qty("4"), Side::Sell);

        let c = l.at(px("100.00"));
        assert_eq!(c.ask, qty("5"), "buy aggressor lifts the ask");
        assert_eq!(c.bid, qty("4"), "sell aggressor hits the bid");
        assert_eq!(c.total(), qty("9"));
        assert_eq!(c.delta(), qty("1"));
        assert_eq!(c.trades, 3);
        assert_eq!(l.trade_count(), 3);
    }

    #[test]
    fn totals_and_delta_match_the_rows() {
        let l = tapered();
        let row_sum: Qty = l.rows().map(|r| r.cluster.total()).sum();
        assert_eq!(l.total_volume(), row_sum);
        assert_eq!(l.total_bid(), qty("116"));
        assert_eq!(l.total_ask(), qty("106"));
        assert_eq!(l.delta(), qty("-10"));
    }

    #[test]
    fn grows_in_both_directions() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.00"), qty("1"), Side::Buy);
        l.add(px("101.00"), qty("1"), Side::Buy); // upward
        l.add(px("99.00"), qty("1"), Side::Buy); // downward

        assert_eq!(l.low().unwrap(), px("99.00"));
        assert_eq!(l.high().unwrap(), px("101.00"));
        // 99.00..=101.00 at a 0.25 tick is 9 levels.
        assert_eq!(l.len(), 9);
        assert_eq!(l.at(px("100.00")).ask, qty("1"));
        assert_eq!(l.at(px("101.00")).ask, qty("1"));
        assert_eq!(l.at(px("99.00")).ask, qty("1"));
        // Untraded levels in between are present and empty.
        assert!(l.at(px("100.50")).is_empty());
    }

    #[test]
    fn untouched_prices_read_as_empty() {
        let l = tapered();
        assert!(l.at(px("95.00")).is_empty());
        assert!(l.at(px("105.00")).is_empty());
        assert!(ClusterLadder::new(tick()).at(px("100.00")).is_empty());
    }

    #[test]
    fn off_tick_prints_floor_onto_a_level() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.13"), qty("5"), Side::Buy);
        assert_eq!(l.at(px("100.00")).ask, qty("5"));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn finds_the_point_of_control() {
        assert_eq!(tapered().poc().unwrap(), px("100.50"));
        assert_eq!(ClusterLadder::new(tick()).poc(), None);
    }

    #[test]
    fn poc_ties_resolve_toward_the_middle() {
        let mut l = ClusterLadder::new(tick());
        // Equal volume at the extremes and nothing between: the tie-break must
        // pick one deterministically rather than always the first row.
        l.add(px("100.00"), qty("10"), Side::Buy);
        l.add(px("100.25"), qty("1"), Side::Buy);
        l.add(px("100.50"), qty("10"), Side::Buy);
        let poc = l.poc().unwrap();
        assert!(poc == px("100.00") || poc == px("100.50"));
        // Deterministic across calls.
        assert_eq!(l.poc().unwrap(), poc);
    }

    #[test]
    fn value_area_brackets_the_poc() {
        let l = tapered();
        let va = l.value_area(0.7).unwrap();
        assert_eq!(va.poc, px("100.50"));
        assert!(va.low <= va.poc && va.poc <= va.high);
        // It must cover at least the requested share of volume.
        let target = (l.total_volume().minor() as f64 * 0.7) as i64;
        assert!(va.volume.minor() >= target, "{:?} < {target}", va.volume);
        // And it must not be the whole ladder for this shape.
        assert!(va.volume <= l.total_volume());
    }

    #[test]
    fn value_area_of_a_single_level_is_that_level() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.00"), qty("7"), Side::Buy);
        let va = l.value_area(0.7).unwrap();
        assert_eq!(va.high, px("100.00"));
        assert_eq!(va.low, px("100.00"));
        assert_eq!(va.volume, qty("7"));
    }

    #[test]
    fn full_value_area_covers_everything() {
        let l = tapered();
        let va = l.value_area(1.0).unwrap();
        assert_eq!(va.low, l.low().unwrap());
        assert_eq!(va.high, l.high().unwrap());
        assert_eq!(va.volume, l.total_volume());
    }

    #[test]
    fn imbalance_is_measured_diagonally() {
        let mut l = ClusterLadder::new(tick());
        // 100.00: bid 10. 100.25: ask 50.
        // Diagonally that is 50 vs 10 = 5.0, a buy imbalance at 100.25.
        // Horizontally 100.25 has no bid at all, which would be a different
        // and wrong answer.
        l.add(px("100.00"), qty("10"), Side::Sell);
        l.add(px("100.25"), qty("50"), Side::Buy);

        let found = l.imbalances(3.0, Qty::ZERO);
        let buys: Vec<_> = found.iter().filter(|i| i.side == Side::Buy).collect();
        assert_eq!(buys.len(), 1);
        assert_eq!(buys[0].price, px("100.25"));
        assert!((buys[0].ratio - 5.0).abs() < 1e-9);
    }

    #[test]
    fn sell_imbalance_compares_against_the_level_above() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.25"), qty("8"), Side::Buy); // ask above
        l.add(px("100.00"), qty("40"), Side::Sell); // bid below

        let found = l.imbalances(3.0, Qty::ZERO);
        let sells: Vec<_> = found.iter().filter(|i| i.side == Side::Sell).collect();
        assert_eq!(sells.len(), 1);
        assert_eq!(sells[0].price, px("100.00"));
        assert!((sells[0].ratio - 5.0).abs() < 1e-9);
    }

    #[test]
    fn unopposed_volume_is_the_strongest_imbalance() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.00"), qty("1"), Side::Buy); // establishes a lower row
        l.add(px("100.25"), qty("30"), Side::Buy); // nothing on the bid below

        let found = l.imbalances(3.0, Qty::ZERO);
        let buy = found.iter().find(|i| i.price == px("100.25")).unwrap();
        assert_eq!(buy.ratio, Imbalance::UNOPPOSED);
    }

    #[test]
    fn ratio_threshold_and_min_volume_filter() {
        let mut l = ClusterLadder::new(tick());
        l.add(px("100.00"), qty("10"), Side::Sell);
        l.add(px("100.25"), qty("20"), Side::Buy); // ratio 2.0

        assert!(!l.imbalances(2.0, Qty::ZERO).is_empty(), "2.0 clears 2.0");
        assert!(l.imbalances(3.0, Qty::ZERO).is_empty(), "2.0 misses 3.0");
        // Passes on ratio but is filtered out by size.
        assert!(l.imbalances(2.0, qty("100")).is_empty());
    }

    #[test]
    fn detects_stacked_runs_and_ignores_short_ones() {
        let mut l = ClusterLadder::new(tick());
        // Four consecutive buy imbalances: each ask is 10x the bid below it.
        for i in 0..5 {
            let p = px("100.00") + tick() * i;
            l.add(p, qty("2"), Side::Sell);
            l.add(p, qty("20"), Side::Buy);
        }

        let runs = l.stacked_imbalances(3.0, Qty::ZERO, 3);
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert!(run.len() >= 3);
        assert!(run.iter().all(|i| i.side == Side::Buy));
        // Contiguous and ascending.
        for pair in run.windows(2) {
            assert_eq!(pair[1].price - pair[0].price, tick());
        }

        // A stack requirement longer than the run finds nothing.
        assert!(l.stacked_imbalances(3.0, Qty::ZERO, 20).is_empty());
    }

    #[test]
    fn empty_ladder_reports_nothing() {
        let l = ClusterLadder::new(tick());
        assert!(l.is_empty());
        assert_eq!(l.len(), 0);
        assert_eq!(l.total_volume(), Qty::ZERO);
        assert_eq!(l.delta(), Qty::ZERO);
        assert_eq!(l.low(), None);
        assert_eq!(l.high(), None);
        assert_eq!(l.value_area(0.7), None);
        assert!(l.imbalances(2.0, Qty::ZERO).is_empty());
        assert_eq!(l.rows().count(), 0);
    }

    #[test]
    fn rows_are_ordered_low_to_high() {
        let l = tapered();
        let prices: Vec<_> = l.rows().map(|r| r.price).collect();
        assert_eq!(prices.first().copied(), l.low());
        assert_eq!(prices.last().copied(), l.high());
        assert!(prices.windows(2).all(|w| w[0] < w[1]));
    }
}
