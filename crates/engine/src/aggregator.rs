//! Bar construction.
//!
//! One aggregator turns a trade stream into bars under a single rule. Which
//! rule matters more than it looks: time bars distort activity (a quiet minute
//! and a violent one get the same width), which is exactly why order-flow
//! traders reach for volume, tick, range and delta bars instead.
//!
//! # Ordering
//!
//! Trades must arrive in non-decreasing timestamp order. A late trade cannot
//! be folded into a bar that has already closed, so out-of-order trades are
//! dropped and counted in [`Aggregator::dropped_out_of_order`] rather than
//! silently corrupting history. Feed adapters are responsible for sequencing.

use atas_core::{Instrument, Price, Qty, Trade, Ts};
use serde::{Deserialize, Serialize};

use crate::bar::Bar;

/// The rule that decides when a bar closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BarSpec {
    /// Close on a fixed wall-clock interval.
    Time {
        /// Bar length in nanoseconds.
        interval_nanos: i64,
    },
    /// Close after a fixed number of trades.
    Tick {
        /// Trades per bar.
        count: u32,
    },
    /// Close once traded volume reaches a threshold.
    Volume {
        /// Volume per bar.
        threshold: Qty,
    },
    /// Close once high minus low reaches a number of ticks.
    Range {
        /// Bar range in ticks.
        ticks: i64,
    },
    /// Close once absolute delta reaches a threshold.
    ///
    /// Delta bars close on aggression rather than on price or clock, so a bar
    /// boundary marks a completed burst of one-sided buying or selling.
    Delta {
        /// Absolute delta per bar.
        threshold: Qty,
    },
}

impl BarSpec {
    /// A time bar of `seconds` length.
    pub const fn seconds(seconds: i64) -> Self {
        BarSpec::Time {
            interval_nanos: seconds * atas_core::time::NANOS_PER_SEC,
        }
    }

    /// A time bar of `minutes` length.
    pub const fn minutes(minutes: i64) -> Self {
        BarSpec::seconds(minutes * 60)
    }

    /// Reject nonsensical configuration before it produces broken bars.
    pub fn validate(&self) -> Result<(), SpecError> {
        match *self {
            BarSpec::Time { interval_nanos } if interval_nanos <= 0 => {
                Err(SpecError::NonPositive("time interval"))
            }
            BarSpec::Tick { count: 0 } => Err(SpecError::NonPositive("tick count")),
            BarSpec::Volume { threshold } if !threshold.is_positive() => {
                Err(SpecError::NonPositive("volume threshold"))
            }
            BarSpec::Range { ticks } if ticks <= 0 => Err(SpecError::NonPositive("range ticks")),
            BarSpec::Delta { threshold } if !threshold.is_positive() => {
                Err(SpecError::NonPositive("delta threshold"))
            }
            _ => Ok(()),
        }
    }
}

/// An invalid [`BarSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    /// A threshold that must be positive was zero or negative.
    #[error("{0} must be positive")]
    NonPositive(&'static str),
}

/// Builds bars from a trade stream under one [`BarSpec`].
#[derive(Debug, Clone)]
pub struct Aggregator {
    tick_size: Price,
    spec: BarSpec,
    current: Option<Bar>,
    /// Bars closed by the most recent trade. Cleared at the top of each call.
    just_closed: Vec<Bar>,
    cum_delta: Qty,
    last_ts: Ts,
    dropped: u64,
}

impl Aggregator {
    /// Create an aggregator for an instrument.
    ///
    /// Returns an error rather than panicking on a bad spec, because specs
    /// come from user-editable workspace files.
    pub fn new(instrument: &Instrument, spec: BarSpec) -> Result<Self, SpecError> {
        Self::with_tick_size(instrument.tick_size, spec)
    }

    /// Create an aggregator from a raw tick size.
    pub fn with_tick_size(tick_size: Price, spec: BarSpec) -> Result<Self, SpecError> {
        spec.validate()?;
        assert!(tick_size.is_positive(), "tick size must be positive");
        Ok(Self {
            tick_size,
            spec,
            current: None,
            just_closed: Vec::new(),
            cum_delta: Qty::ZERO,
            last_ts: Ts::EPOCH,
            dropped: 0,
        })
    }

    /// Fold a trade in, returning any bars it closed.
    ///
    /// The returned slice is usually empty or one bar; it is a slice so that
    /// rules able to close several bars from one print have somewhere to put
    /// them.
    pub fn on_trade(&mut self, trade: &Trade) -> &[Bar] {
        self.just_closed.clear();

        if trade.ts < self.last_ts {
            self.dropped += 1;
            return &self.just_closed;
        }
        self.last_ts = trade.ts;

        // Time bars are the one rule where the boundary is decided before the
        // trade is applied: a trade belonging to the next bucket must not
        // touch the current bar at all.
        if let BarSpec::Time { interval_nanos } = self.spec {
            let bucket = trade.ts.floor_to(interval_nanos);
            let rolled = self.current.as_ref().is_some_and(|b| b.open_ts != bucket);
            if rolled {
                self.close_current();
            }
            if self.current.is_none() {
                self.current = Some(Bar::open(bucket, trade, self.tick_size));
                self.cum_delta += trade.signed_qty();
                return &self.just_closed;
            }
        }

        match &mut self.current {
            Some(bar) => bar.apply(trade),
            None => self.current = Some(Bar::open(trade.ts, trade, self.tick_size)),
        }
        self.cum_delta += trade.signed_qty();

        if self.should_close() {
            self.close_current();
        }
        &self.just_closed
    }

    /// Whether the forming bar has satisfied its closing rule.
    fn should_close(&self) -> bool {
        let Some(bar) = self.current.as_ref() else {
            return false;
        };
        match self.spec {
            // Handled before the trade is applied.
            BarSpec::Time { .. } => false,
            BarSpec::Tick { count } => bar.trades >= count,
            BarSpec::Volume { threshold } => bar.volume >= threshold,
            BarSpec::Range { ticks } => {
                bar.range().minor() >= self.tick_size.minor().saturating_mul(ticks)
            }
            BarSpec::Delta { threshold } => bar.delta().abs() >= threshold,
        }
    }

    /// Close the forming bar into the just-closed buffer.
    fn close_current(&mut self) {
        if let Some(mut bar) = self.current.take() {
            bar.close();
            self.just_closed.push(bar);
        }
    }

    /// Close the forming bar, if any. Use at session end or on disconnect, so
    /// a partial bar is not left dangling.
    pub fn flush(&mut self) -> Option<Bar> {
        self.current.take().map(|mut bar| {
            bar.close();
            bar
        })
    }

    /// The bar currently being formed.
    #[inline]
    pub fn current(&self) -> Option<&Bar> {
        self.current.as_ref()
    }

    /// Session cumulative delta, including the forming bar.
    #[inline]
    pub fn cum_delta(&self) -> Qty {
        self.cum_delta
    }

    /// The rule this aggregator applies.
    #[inline]
    pub fn spec(&self) -> BarSpec {
        self.spec
    }

    /// How many trades were dropped for arriving out of order.
    ///
    /// A non-zero value in production means the feed adapter is not sequencing
    /// correctly, and the resulting history has holes.
    #[inline]
    pub fn dropped_out_of_order(&self) -> u64 {
        self.dropped
    }

    /// Reset all state, keeping the configuration. Use when switching
    /// instruments or restarting a session.
    pub fn reset(&mut self) {
        self.current = None;
        self.just_closed.clear();
        self.cum_delta = Qty::ZERO;
        self.last_ts = Ts::EPOCH;
        self.dropped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atas_core::{time::NANOS_PER_SEC, Side, Venue};

    fn px(s: &str) -> Price {
        Price::parse(s).unwrap()
    }
    fn qty(s: &str) -> Qty {
        Qty::parse(s).unwrap()
    }
    fn instrument() -> Instrument {
        Instrument::spot("TEST", Venue::Sim, px("0.25"), qty("1"))
    }
    fn trade(ms: i64, price: &str, q: &str, side: Side) -> Trade {
        Trade::new(Ts::from_millis(ms), px(price), qty(q), side, 0)
    }
    fn agg(spec: BarSpec) -> Aggregator {
        Aggregator::new(&instrument(), spec).unwrap()
    }

    #[test]
    fn rejects_degenerate_specs() {
        assert!(BarSpec::Time { interval_nanos: 0 }.validate().is_err());
        assert!(BarSpec::Tick { count: 0 }.validate().is_err());
        assert!(BarSpec::Volume {
            threshold: Qty::ZERO
        }
        .validate()
        .is_err());
        assert!(BarSpec::Range { ticks: -1 }.validate().is_err());
        assert!(BarSpec::minutes(1).validate().is_ok());
        assert!(Aggregator::new(&instrument(), BarSpec::Tick { count: 0 }).is_err());
    }

    #[test]
    fn tick_bars_close_on_trade_count() {
        let mut a = agg(BarSpec::Tick { count: 3 });
        assert!(a.on_trade(&trade(1, "100.00", "1", Side::Buy)).is_empty());
        assert!(a.on_trade(&trade(2, "100.25", "1", Side::Buy)).is_empty());

        let closed = a.on_trade(&trade(3, "100.50", "1", Side::Sell));
        assert_eq!(closed.len(), 1);
        let bar = &closed[0];
        assert!(bar.closed);
        assert_eq!(bar.trades, 3);
        assert_eq!(bar.open, px("100.00"));
        assert_eq!(bar.close, px("100.50"));
        assert_eq!(bar.volume, qty("3"));
        // A fresh bar has not started until the next trade arrives.
        assert!(a.current().is_none());

        a.on_trade(&trade(4, "100.75", "1", Side::Buy));
        assert_eq!(a.current().unwrap().trades, 1);
    }

    #[test]
    fn volume_bars_close_on_threshold() {
        let mut a = agg(BarSpec::Volume {
            threshold: qty("10"),
        });
        assert!(a.on_trade(&trade(1, "100.00", "4", Side::Buy)).is_empty());
        assert!(a.on_trade(&trade(2, "100.00", "5", Side::Buy)).is_empty());

        let closed = a.on_trade(&trade(3, "100.00", "3", Side::Buy));
        assert_eq!(closed.len(), 1);
        // Volume may overshoot: the trade that crosses the line is not split.
        assert_eq!(closed[0].volume, qty("12"));
    }

    #[test]
    fn range_bars_close_on_tick_span() {
        let mut a = agg(BarSpec::Range { ticks: 4 }); // 4 * 0.25 = 1.00
        assert!(a.on_trade(&trade(1, "100.00", "1", Side::Buy)).is_empty());
        assert!(a.on_trade(&trade(2, "100.75", "1", Side::Buy)).is_empty());

        let closed = a.on_trade(&trade(3, "101.00", "1", Side::Buy));
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].range(), px("1.00"));
    }

    #[test]
    fn delta_bars_close_on_absolute_aggression() {
        let mut a = agg(BarSpec::Delta {
            threshold: qty("5"),
        });
        assert!(a.on_trade(&trade(1, "100.00", "3", Side::Buy)).is_empty());
        let closed = a.on_trade(&trade(2, "100.00", "3", Side::Buy));
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].delta(), qty("6"));

        // Selling closes a bar just as readily as buying.
        let mut b = agg(BarSpec::Delta {
            threshold: qty("5"),
        });
        assert!(b.on_trade(&trade(1, "100.00", "2", Side::Sell)).is_empty());
        let closed = b.on_trade(&trade(2, "100.00", "4", Side::Sell));
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].delta(), qty("-6"));
    }

    #[test]
    fn delta_bars_do_not_close_when_flow_is_balanced() {
        let mut a = agg(BarSpec::Delta {
            threshold: qty("5"),
        });
        for i in 0..10 {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            assert!(a.on_trade(&trade(i, "100.00", "1", side)).is_empty());
        }
        // Ten trades, zero net delta, still one open bar.
        assert_eq!(a.current().unwrap().trades, 10);
        assert_eq!(a.current().unwrap().delta(), Qty::ZERO);
    }

    #[test]
    fn time_bars_bucket_by_boundary() {
        let mut a = agg(BarSpec::seconds(1));
        // Two trades inside the same second.
        assert!(a.on_trade(&trade(1_000, "100.00", "1", Side::Buy)).is_empty());
        assert!(a.on_trade(&trade(1_500, "100.25", "1", Side::Buy)).is_empty());

        // A trade in the next second closes the first bar.
        let closed = a.on_trade(&trade(2_000, "100.50", "1", Side::Buy));
        assert_eq!(closed.len(), 1);
        let bar = &closed[0];
        assert_eq!(bar.trades, 2);
        // The bar opens on the bucket boundary, not the first trade's stamp.
        assert_eq!(bar.open_ts, Ts::from_millis(1_000));
        assert_eq!(bar.close_ts, Ts::from_millis(1_500));

        // The boundary-crossing trade belongs to the new bar only.
        let current = a.current().unwrap();
        assert_eq!(current.trades, 1);
        assert_eq!(current.open, px("100.50"));
        assert_eq!(current.open_ts, Ts::from_millis(2_000));
    }

    #[test]
    fn time_bars_skip_empty_buckets() {
        let mut a = agg(BarSpec::seconds(1));
        a.on_trade(&trade(1_000, "100.00", "1", Side::Buy));
        // Nothing trades for five seconds. No phantom empty bars are emitted.
        let closed = a.on_trade(&trade(6_000, "100.00", "1", Side::Buy));
        assert_eq!(closed.len(), 1);
        assert_eq!(a.current().unwrap().open_ts, Ts::from_millis(6_000));
    }

    #[test]
    fn first_time_bar_opens_on_its_bucket() {
        let mut a = agg(BarSpec::seconds(60));
        a.on_trade(&trade(1_700_000_077_500, "100.00", "1", Side::Buy));
        assert_eq!(
            a.current().unwrap().open_ts,
            Ts::from_millis(1_700_000_040_000)
        );
    }

    #[test]
    fn out_of_order_trades_are_dropped_not_applied() {
        let mut a = agg(BarSpec::Tick { count: 100 });
        a.on_trade(&trade(5_000, "100.00", "1", Side::Buy));
        assert!(a.on_trade(&trade(4_000, "999.00", "50", Side::Buy)).is_empty());

        assert_eq!(a.dropped_out_of_order(), 1);
        let bar = a.current().unwrap();
        assert_eq!(bar.trades, 1, "the late trade must not be folded in");
        assert_eq!(bar.high, px("100.00"), "and must not move the high");
        assert_eq!(bar.volume, qty("1"));
    }

    #[test]
    fn same_timestamp_trades_are_accepted() {
        // Venues routinely stamp a swept book with one millisecond.
        let mut a = agg(BarSpec::Tick { count: 100 });
        a.on_trade(&trade(5_000, "100.00", "1", Side::Buy));
        a.on_trade(&trade(5_000, "100.25", "1", Side::Buy));
        assert_eq!(a.dropped_out_of_order(), 0);
        assert_eq!(a.current().unwrap().trades, 2);
    }

    #[test]
    fn cumulative_delta_runs_across_bars() {
        let mut a = agg(BarSpec::Tick { count: 2 });
        a.on_trade(&trade(1, "100.00", "5", Side::Buy));
        a.on_trade(&trade(2, "100.00", "2", Side::Sell)); // closes bar, +3
        assert_eq!(a.cum_delta(), qty("3"));

        a.on_trade(&trade(3, "100.00", "1", Side::Sell)); // -1
        assert_eq!(a.cum_delta(), qty("2"));
    }

    #[test]
    fn flush_closes_the_forming_bar_once() {
        let mut a = agg(BarSpec::Tick { count: 100 });
        a.on_trade(&trade(1, "100.00", "1", Side::Buy));

        let bar = a.flush().unwrap();
        assert!(bar.closed);
        assert_eq!(bar.trades, 1);
        assert!(a.current().is_none());
        assert!(a.flush().is_none(), "flushing twice yields nothing");
    }

    #[test]
    fn reset_clears_state_but_keeps_the_spec() {
        let mut a = agg(BarSpec::Tick { count: 3 });
        a.on_trade(&trade(1, "100.00", "1", Side::Buy));
        a.on_trade(&trade(0, "100.00", "1", Side::Buy)); // dropped
        a.reset();

        assert!(a.current().is_none());
        assert_eq!(a.cum_delta(), Qty::ZERO);
        assert_eq!(a.dropped_out_of_order(), 0);
        assert_eq!(a.spec(), BarSpec::Tick { count: 3 });
        // And an old timestamp is accepted again after the reset.
        assert!(a.on_trade(&trade(1, "100.00", "1", Side::Buy)).is_empty());
        assert_eq!(a.current().unwrap().trades, 1);
    }

    #[test]
    fn bars_carry_their_cluster_ladder() {
        let mut a = agg(BarSpec::Tick { count: 3 });
        a.on_trade(&trade(1, "100.00", "10", Side::Sell));
        a.on_trade(&trade(2, "100.25", "40", Side::Buy));
        let closed = a.on_trade(&trade(3, "100.25", "5", Side::Buy));

        let bar = &closed[0];
        assert_eq!(bar.clusters.at(px("100.00")).bid, qty("10"));
        assert_eq!(bar.clusters.at(px("100.25")).ask, qty("45"));
        assert_eq!(bar.poc().unwrap(), px("100.25"));
        // 45 on the ask against 10 on the bid one tick below.
        let imb = bar.clusters.imbalances(3.0, Qty::ZERO);
        assert_eq!(imb.len(), 1);
        assert_eq!(imb[0].price, px("100.25"));
    }

    #[test]
    fn helper_constructors_compute_the_right_interval() {
        assert_eq!(
            BarSpec::minutes(5),
            BarSpec::Time {
                interval_nanos: 300 * NANOS_PER_SEC
            }
        );
    }
}
