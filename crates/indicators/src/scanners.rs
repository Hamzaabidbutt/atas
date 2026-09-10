//! Scanners: the tools that point at what to look at.
//!
//! A trader cannot watch every print on every instrument. These reduce the
//! stream to the events matching conditions the trader specified, which is the
//! difference between a chart you stare at and a chart that tells you when
//! something happened.

use atas_core::{Price, Qty, Side, Trade, Ts};
use atas_engine::{Bar, Imbalance};
use serde::{Deserialize, Serialize};

use crate::traits::TradeIndicator;

/// A single print large enough to be worth seeing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BigTrade {
    /// When it printed.
    pub ts: Ts,
    /// Where it printed.
    pub price: Price,
    /// How large it was.
    pub qty: Qty,
    /// Which side was aggressive.
    pub aggressor: Side,
}

/// Flags individual prints at or above a size threshold.
///
/// Deliberately trade-level rather than bar-level: a single 400-lot inside a
/// bar that traded 4,000 is invisible in the bar's totals, and it is exactly
/// what the trader wanted to see.
#[derive(Debug, Clone)]
pub struct BigTrades {
    threshold: Qty,
    capacity: usize,
    recent: std::collections::VecDeque<BigTrade>,
    seen: u64,
}

impl BigTrades {
    /// Flag prints of `threshold` or larger, keeping the last `capacity`.
    pub fn new(threshold: Qty, capacity: usize) -> Self {
        assert!(threshold.is_positive(), "threshold must be positive");
        assert!(capacity > 0, "capacity must be positive");
        Self {
            threshold,
            capacity,
            recent: std::collections::VecDeque::with_capacity(capacity),
            seen: 0,
        }
    }

    /// The most recent flagged prints, newest last.
    pub fn recent(&self) -> impl Iterator<Item = &BigTrade> {
        self.recent.iter()
    }

    /// How many prints have been flagged in total.
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// The size threshold in force.
    pub fn threshold(&self) -> Qty {
        self.threshold
    }
}

impl TradeIndicator for BigTrades {
    type Value = Option<BigTrade>;

    fn name(&self) -> &str {
        "Big Trades"
    }

    fn update(&mut self, trade: &Trade) -> Option<BigTrade> {
        if trade.qty < self.threshold {
            return None;
        }
        let big = BigTrade {
            ts: trade.ts,
            price: trade.price,
            qty: trade.qty,
            aggressor: trade.aggressor,
        };
        if self.recent.len() == self.capacity {
            self.recent.pop_front();
        }
        self.recent.push_back(big);
        self.seen += 1;
        Some(big)
    }

    fn value(&self) -> Option<Option<BigTrade>> {
        Some(self.recent.back().copied())
    }

    fn reset(&mut self) {
        self.recent.clear();
        self.seen = 0;
    }
}

/// Conditions a bar must satisfy to be flagged by [`ClusterSearch`].
#[derive(Debug, Clone, Default)]
pub struct ClusterCriteria {
    /// Minimum total volume in the bar.
    pub min_volume: Option<Qty>,
    /// Minimum volume at any single price level.
    pub min_level_volume: Option<Qty>,
    /// Minimum absolute bar delta.
    pub min_abs_delta: Option<Qty>,
    /// Require a stacked imbalance of at least this many consecutive levels.
    pub min_imbalance_stack: Option<usize>,
    /// Ratio defining an imbalance. Defaults to 3.0 when a stack is required.
    pub imbalance_ratio: Option<f64>,
    /// Minimum volume for a level to count toward an imbalance.
    pub imbalance_min_volume: Option<Qty>,
}

impl ClusterCriteria {
    /// Criteria matching nothing until something is set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Require a minimum bar volume.
    pub fn min_volume(mut self, qty: Qty) -> Self {
        self.min_volume = Some(qty);
        self
    }

    /// Require a minimum volume at a single level.
    pub fn min_level_volume(mut self, qty: Qty) -> Self {
        self.min_level_volume = Some(qty);
        self
    }

    /// Require a minimum absolute delta.
    pub fn min_abs_delta(mut self, qty: Qty) -> Self {
        self.min_abs_delta = Some(qty);
        self
    }

    /// Require a stack of `levels` consecutive same-side imbalances.
    pub fn min_imbalance_stack(mut self, levels: usize) -> Self {
        self.min_imbalance_stack = Some(levels);
        self
    }

    /// Set the ratio that defines an imbalance.
    pub fn imbalance_ratio(mut self, ratio: f64) -> Self {
        self.imbalance_ratio = Some(ratio);
        self
    }

    /// Whether any condition has been set.
    pub fn is_empty(&self) -> bool {
        self.min_volume.is_none()
            && self.min_level_volume.is_none()
            && self.min_abs_delta.is_none()
            && self.min_imbalance_stack.is_none()
    }
}

/// What a bar matched on.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterHit {
    /// When the matching bar opened.
    pub open_ts: Ts,
    /// Total volume in the bar.
    pub volume: Qty,
    /// The bar's delta.
    pub delta: Qty,
    /// Heaviest single level, if it was the reason for the match.
    pub heaviest_level: Option<(Price, Qty)>,
    /// Stacked imbalances found, if a stack was required.
    pub stacks: Vec<Vec<Imbalance>>,
}

/// Scans completed bars for cluster conditions.
///
/// Where [`BigTrades`] finds one large print, this finds structure: a level
/// that absorbed heavy volume, or a run of imbalances showing one side kept
/// paying up and kept getting filled.
#[derive(Debug, Clone)]
pub struct ClusterSearch {
    criteria: ClusterCriteria,
    hits: Vec<ClusterHit>,
    scanned: u64,
}

impl ClusterSearch {
    /// A scanner for the given criteria.
    pub fn new(criteria: ClusterCriteria) -> Self {
        Self {
            criteria,
            hits: Vec::new(),
            scanned: 0,
        }
    }

    /// Test one bar, recording and returning a hit if it matches.
    ///
    /// Empty criteria match nothing: a scanner that fires on every bar is
    /// worse than no scanner, because the trader stops looking at it.
    pub fn scan(&mut self, bar: &Bar) -> Option<ClusterHit> {
        self.scanned += 1;
        if self.criteria.is_empty() {
            return None;
        }

        if let Some(min) = self.criteria.min_volume {
            if bar.volume < min {
                return None;
            }
        }
        if let Some(min) = self.criteria.min_abs_delta {
            if bar.delta().abs() < min {
                return None;
            }
        }

        let heaviest = bar
            .clusters
            .rows()
            .max_by_key(|r| r.cluster.total())
            .map(|r| (r.price, r.cluster.total()));

        if let Some(min) = self.criteria.min_level_volume {
            match heaviest {
                Some((_, total)) if total >= min => {}
                _ => return None,
            }
        }

        let mut stacks = Vec::new();
        if let Some(min_run) = self.criteria.min_imbalance_stack {
            let ratio = self.criteria.imbalance_ratio.unwrap_or(3.0);
            let min_vol = self.criteria.imbalance_min_volume.unwrap_or(Qty::ZERO);
            stacks = bar.clusters.stacked_imbalances(ratio, min_vol, min_run);
            if stacks.is_empty() {
                return None;
            }
        }

        let hit = ClusterHit {
            open_ts: bar.open_ts,
            volume: bar.volume,
            delta: bar.delta(),
            heaviest_level: self.criteria.min_level_volume.and(heaviest),
            stacks,
        };
        self.hits.push(hit.clone());
        Some(hit)
    }

    /// Every hit so far.
    pub fn hits(&self) -> &[ClusterHit] {
        &self.hits
    }

    /// How many bars have been tested.
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    /// Clear recorded hits and counters.
    pub fn reset(&mut self) {
        self.hits.clear();
        self.scanned = 0;
    }
}

/// Trades per second over a rolling window.
///
/// Speed of tape is how a trader senses urgency: the same volume delivered in
/// two seconds instead of thirty means something different.
#[derive(Debug, Clone)]
pub struct SpeedOfTape {
    window_nanos: i64,
    timestamps: std::collections::VecDeque<Ts>,
    last: Option<f64>,
}

impl SpeedOfTape {
    /// Measure over a rolling window.
    pub fn new(window_nanos: i64) -> Self {
        assert!(window_nanos > 0, "window must be positive");
        Self {
            window_nanos,
            timestamps: std::collections::VecDeque::new(),
            last: None,
        }
    }

    /// Trades currently inside the window.
    pub fn count(&self) -> usize {
        self.timestamps.len()
    }
}

impl TradeIndicator for SpeedOfTape {
    type Value = f64;

    fn name(&self) -> &str {
        "Speed of Tape"
    }

    fn update(&mut self, trade: &Trade) -> f64 {
        self.timestamps.push_back(trade.ts);
        let cutoff = trade.ts - self.window_nanos;
        while self.timestamps.front().is_some_and(|&t| t < cutoff) {
            self.timestamps.pop_front();
        }

        let per_second = self.timestamps.len() as f64
            / (self.window_nanos as f64 / atas_core::time::NANOS_PER_SEC as f64);
        self.last = Some(per_second);
        per_second
    }

    fn value(&self) -> Option<f64> {
        self.last
    }

    fn reset(&mut self) {
        self.timestamps.clear();
        self.last = None;
    }
}
