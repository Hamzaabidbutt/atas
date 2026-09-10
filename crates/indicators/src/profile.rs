//! Session volume profile and TPO.
//!
//! A volume profile answers "how much traded at each price"; a TPO profile
//! answers "how long did price spend at each level". They disagree often, and
//! the disagreement is the information: heavy volume in a narrow TPO band is
//! absorption, thin volume across a wide band is a market that moved through
//! without interest.

use std::collections::BTreeSet;

use atas_core::{Price, Qty, Ts};
use atas_engine::{ClusterLadder, ClusterRow, ValueArea};

use crate::traits::BarIndicator;

/// Share of volume conventionally enclosed by the value area.
pub const DEFAULT_VALUE_AREA: f64 = 0.70;

/// Volume traded at each price across a session.
///
/// Built on the same [`ClusterLadder`] the engine uses for bars, so POC and
/// value-area computation is shared rather than reimplemented with subtly
/// different tie-breaking.
#[derive(Debug, Clone)]
pub struct SessionProfile {
    ladder: ClusterLadder,
    session_start: Option<Ts>,
    session_end: Option<Ts>,
    bars: u64,
}

impl SessionProfile {
    /// An empty profile for an instrument's tick size.
    pub fn new(tick_size: Price) -> Self {
        Self {
            ladder: ClusterLadder::new(tick_size),
            session_start: None,
            session_end: None,
            bars: 0,
        }
    }

    /// The accumulated ladder.
    pub fn ladder(&self) -> &ClusterLadder {
        &self.ladder
    }

    /// Total volume in the session.
    pub fn volume(&self) -> Qty {
        self.ladder.total_volume()
    }

    /// Session delta.
    pub fn delta(&self) -> Qty {
        self.ladder.delta()
    }

    /// The most-traded price.
    pub fn poc(&self) -> Option<Price> {
        self.ladder.poc()
    }

    /// The value area enclosing `DEFAULT_VALUE_AREA` of volume.
    pub fn value_area(&self) -> Option<ValueArea> {
        self.ladder.value_area(DEFAULT_VALUE_AREA)
    }

    /// The value area for a custom share of volume.
    pub fn value_area_of(&self, fraction: f64) -> Option<ValueArea> {
        self.ladder.value_area(fraction)
    }

    /// Every price level, low to high.
    pub fn rows(&self) -> impl Iterator<Item = ClusterRow> + '_ {
        self.ladder.rows()
    }

    /// Timestamps of the first and last bar folded in.
    pub fn span(&self) -> Option<(Ts, Ts)> {
        Some((self.session_start?, self.session_end?))
    }

    /// Prices that traded in only one bar.
    ///
    /// Single prints mark levels the market rejected quickly, and they tend to
    /// act as magnets when price returns to them.
    pub fn single_prints(&self) -> Vec<Price> {
        self.ladder
            .rows()
            .filter(|r| r.cluster.trades == 1)
            .map(|r| r.price)
            .collect()
    }
}

impl BarIndicator for SessionProfile {
    type Value = Qty;

    fn name(&self) -> &str {
        "Volume Profile"
    }

    fn update(&mut self, bar: &atas_engine::Bar) -> Qty {
        for row in bar.clusters.rows() {
            if row.cluster.bid.is_positive() {
                self.ladder
                    .add(row.price, row.cluster.bid, atas_core::Side::Sell);
            }
            if row.cluster.ask.is_positive() {
                self.ladder
                    .add(row.price, row.cluster.ask, atas_core::Side::Buy);
            }
        }
        self.session_start.get_or_insert(bar.open_ts);
        self.session_end = Some(bar.close_ts);
        self.bars += 1;
        self.ladder.total_volume()
    }

    fn value(&self) -> Option<Qty> {
        (self.bars > 0).then(|| self.ladder.total_volume())
    }

    fn reset(&mut self) {
        let tick = self.ladder.tick_size();
        *self = Self::new(tick);
    }
}

/// A time-price-opportunity profile.
///
/// Each bracket that trades at a price contributes one TPO to that level,
/// regardless of how much volume changed hands. Distribution shape, not size,
/// is what this measures.
#[derive(Debug, Clone)]
pub struct TpoProfile {
    tick_size: Price,
    bracket_nanos: i64,
    /// Price levels touched, per bracket index, so a bracket cannot count the
    /// same level twice however many bars fall inside it.
    current_bracket: Option<i64>,
    current_levels: BTreeSet<i64>,
    counts: std::collections::BTreeMap<i64, u32>,
    brackets: u32,
}

impl TpoProfile {
    /// A profile with the given bracket length. Thirty minutes is the
    /// convention inherited from the CBOT market profile.
    pub fn new(tick_size: Price, bracket_nanos: i64) -> Self {
        assert!(tick_size.is_positive(), "tick size must be positive");
        assert!(bracket_nanos > 0, "bracket length must be positive");
        Self {
            tick_size,
            bracket_nanos,
            current_bracket: None,
            current_levels: BTreeSet::new(),
            counts: std::collections::BTreeMap::new(),
            brackets: 0,
        }
    }

    /// A profile with thirty-minute brackets.
    pub fn half_hourly(tick_size: Price) -> Self {
        Self::new(tick_size, 30 * 60 * atas_core::time::NANOS_PER_SEC)
    }

    /// Number of completed brackets, plus the one in progress.
    pub fn bracket_count(&self) -> u32 {
        self.brackets
    }

    /// TPO count at a price.
    pub fn count_at(&self, price: Price) -> u32 {
        self.counts
            .get(&price.tick_index(self.tick_size))
            .copied()
            .unwrap_or(0)
    }

    /// Every level with a TPO count, low to high.
    pub fn rows(&self) -> impl Iterator<Item = (Price, u32)> + '_ {
        self.counts
            .iter()
            .map(|(&idx, &count)| (Price::from_tick_index(idx, self.tick_size), count))
    }

    /// The level with the most TPOs.
    pub fn poc(&self) -> Option<Price> {
        self.counts
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(&idx, _)| Price::from_tick_index(idx, self.tick_size))
    }

    /// Flush the bracket in progress into the counts.
    fn close_bracket(&mut self) {
        if self.current_bracket.is_none() {
            return;
        }
        for level in std::mem::take(&mut self.current_levels) {
            *self.counts.entry(level).or_insert(0) += 1;
        }
        self.brackets += 1;
    }

    /// Record a bar's price range against its bracket.
    pub fn add_bar(&mut self, bar: &atas_engine::Bar) {
        let bracket = bar.open_ts.nanos().div_euclid(self.bracket_nanos);
        if self.current_bracket != Some(bracket) {
            self.close_bracket();
            self.current_bracket = Some(bracket);
        }

        let low = bar.low.tick_index(self.tick_size);
        let high = bar.high.tick_index(self.tick_size);
        for level in low..=high {
            self.current_levels.insert(level);
        }
    }

    /// Close the bracket in progress. Call at session end so the final
    /// bracket is counted.
    pub fn finish(&mut self) {
        self.close_bracket();
        self.current_bracket = None;
    }
}

impl BarIndicator for TpoProfile {
    type Value = u32;

    fn name(&self) -> &str {
        "TPO Profile"
    }

    fn update(&mut self, bar: &atas_engine::Bar) -> u32 {
        self.add_bar(bar);
        self.brackets
    }

    fn value(&self) -> Option<u32> {
        (self.current_bracket.is_some() || self.brackets > 0).then_some(self.brackets)
    }

    fn reset(&mut self) {
        *self = Self::new(self.tick_size, self.bracket_nanos);
    }
}
