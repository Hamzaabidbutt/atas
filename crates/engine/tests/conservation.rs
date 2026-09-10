//! End-to-end invariants over a large synthetic trade stream.
//!
//! Unit tests check individual rules; these check the properties that must
//! hold no matter which rule is in play. If volume or delta can leak during
//! aggregation, every downstream number — profile, CVD, PnL — is wrong, and
//! the leak would only show up as a slow drift that is painful to trace.

use atas_core::{Instrument, Price, Qty, Side, Trade, Ts, Venue};
use atas_engine::{Aggregator, BarSpec};

/// Deterministic trade stream. Seeded so a failure is always reproducible.
fn stream(count: usize) -> Vec<Trade> {
    let mut seed: u64 = 0x5EED_1234_ABCD_0001;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let tick_minor = Price::parse("0.25").unwrap().minor();
    let mut price_index = 400_000i64; // arbitrary starting tick index
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let r = next();
        // Random walk of -2..=2 ticks.
        let step = (r % 5) as i64 - 2;
        price_index += step;
        let price = Price::from_minor(price_index * tick_minor);
        // Sizes 1..=50 in whole units.
        let qty = Qty::from_units(((r >> 8) % 50 + 1) as i64);
        let aggressor = if (r >> 16) % 2 == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        // Strictly non-decreasing timestamps, ~1ms apart with ties.
        let ts = Ts::from_millis((i / 2) as i64);
        out.push(Trade::new(ts, price, qty, aggressor, i as u64));
    }
    out
}

fn instrument() -> Instrument {
    Instrument::spot(
        "SIM",
        Venue::Sim,
        Price::parse("0.25").unwrap(),
        Qty::parse("1").unwrap(),
    )
}

/// Run a stream through one spec and assert nothing leaked.
fn assert_conserved(spec: BarSpec, trades: &[Trade]) {
    let mut agg = Aggregator::new(&instrument(), spec).unwrap();

    let mut bar_volume = Qty::ZERO;
    let mut bar_trades = 0u64;
    let mut bar_delta = Qty::ZERO;
    let mut closed_bars = 0u64;

    for trade in trades {
        for bar in agg.on_trade(trade) {
            assert!(bar.closed, "{spec:?}: emitted bar must be marked closed");
            assert!(
                bar.low <= bar.high,
                "{spec:?}: low {} above high {}",
                bar.low,
                bar.high
            );
            assert!(bar.trades > 0, "{spec:?}: emitted an empty bar");
            assert_eq!(
                bar.volume,
                bar.clusters.total_volume(),
                "{spec:?}: bar volume disagrees with its ladder"
            );
            assert_eq!(
                bar.delta(),
                bar.clusters.total_ask() - bar.clusters.total_bid(),
                "{spec:?}: delta disagrees with the ladder"
            );
            // Every traded price must fall inside the bar's own range.
            if let (Some(lo), Some(hi)) = (bar.clusters.low(), bar.clusters.high()) {
                assert!(lo >= bar.low && hi <= bar.high, "{spec:?}: ladder outside OHLC");
            }

            bar_volume += bar.volume;
            bar_trades += bar.trades as u64;
            bar_delta += bar.delta();
            closed_bars += 1;
        }
    }

    if let Some(bar) = agg.flush() {
        bar_volume += bar.volume;
        bar_trades += bar.trades as u64;
        bar_delta += bar.delta();
        closed_bars += 1;
    }

    let expected_volume: Qty = trades.iter().map(|t| t.qty).sum();
    let expected_delta: Qty = trades.iter().map(|t| t.signed_qty()).sum();

    assert_eq!(agg.dropped_out_of_order(), 0, "{spec:?}: dropped trades");
    assert_eq!(bar_trades, trades.len() as u64, "{spec:?}: trade count leak");
    assert_eq!(bar_volume, expected_volume, "{spec:?}: volume leak");
    assert_eq!(bar_delta, expected_delta, "{spec:?}: delta leak");
    assert!(closed_bars > 0, "{spec:?}: produced no bars at all");
}

#[test]
fn every_bar_rule_conserves_volume_and_delta() {
    let trades = stream(200_000);

    for spec in [
        BarSpec::seconds(1),
        BarSpec::minutes(1),
        BarSpec::Tick { count: 100 },
        BarSpec::Volume {
            threshold: Qty::from_units(500),
        },
        BarSpec::Range { ticks: 8 },
        BarSpec::Delta {
            threshold: Qty::from_units(200),
        },
    ] {
        assert_conserved(spec, &trades);
    }
}

#[test]
fn cumulative_delta_matches_the_stream() {
    let trades = stream(50_000);
    let mut agg = Aggregator::new(&instrument(), BarSpec::Tick { count: 250 }).unwrap();
    for trade in &trades {
        agg.on_trade(trade);
    }
    let expected: Qty = trades.iter().map(|t| t.signed_qty()).sum();
    assert_eq!(agg.cum_delta(), expected);
}

#[test]
fn ladders_stay_bounded_on_a_wandering_price() {
    // A random walk over 200k trades must not produce an unbounded ladder for
    // a bounded bar rule: a dense Vec that grew with the walk instead of the
    // bar would exhaust memory in a live session.
    let trades = stream(200_000);
    let mut agg = Aggregator::new(&instrument(), BarSpec::Range { ticks: 8 }).unwrap();
    let mut widest = 0usize;
    for trade in &trades {
        for bar in agg.on_trade(trade) {
            widest = widest.max(bar.clusters.len());
        }
    }
    // A range bar closes at 8 ticks, so its ladder spans at most a few more.
    assert!(widest <= 16, "range bar ladder grew to {widest} rows");
}
