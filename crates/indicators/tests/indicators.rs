//! Indicator tests, checked against hand-computed values.

use atas_core::{Instrument, Price, Qty, Side, Trade, Ts, Venue};
use atas_engine::{Aggregator, Bar, BarSpec};
use atas_indicators::*;

fn px(s: &str) -> Price {
    Price::parse(s).unwrap()
}
fn qty(s: &str) -> Qty {
    Qty::parse(s).unwrap()
}
fn tick() -> Price {
    px("0.25")
}
fn instrument() -> Instrument {
    Instrument::spot("TEST", Venue::Sim, tick(), qty("1"))
}

/// Build a one-bar ladder from `(price, qty, side)` triples.
fn bar_from(ms: i64, prints: &[(&str, &str, Side)]) -> Bar {
    let mut agg = Aggregator::new(&instrument(), BarSpec::Tick { count: u32::MAX }).unwrap();
    for (i, (p, q, side)) in prints.iter().enumerate() {
        agg.on_trade(&Trade::new(
            Ts::from_millis(ms + i as i64),
            px(p),
            qty(q),
            *side,
            i as u64,
        ));
    }
    agg.flush().expect("prints must produce a bar")
}

// --- CVD ------------------------------------------------------------------

#[test]
fn cvd_accumulates_bar_delta() {
    let mut cvd = Cvd::new();
    assert_eq!(cvd.value(), None, "no value before any bar");

    // +5 buy, -2 sell => delta +3
    let bar = bar_from(0, &[("100.00", "5", Side::Buy), ("100.00", "2", Side::Sell)]);
    assert_eq!(cvd.update(&bar), qty("3"));

    // delta -4
    let bar = bar_from(100, &[("100.00", "4", Side::Sell)]);
    assert_eq!(cvd.update(&bar), qty("-1"));
    assert_eq!(cvd.total(), qty("-1"));
    assert_eq!(cvd.bars(), 2);
    assert_eq!(cvd.value(), Some(qty("-1")));
}

#[test]
fn cvd_flags_a_bearish_divergence() {
    let mut cvd = Cvd::new();
    // First push: a high at 101 on strong buying.
    cvd.update(&bar_from(0, &[("101.00", "100", Side::Buy)]));
    assert_eq!(cvd.divergence(), None);

    // Heavy selling drags CVD down without taking out the high.
    cvd.update(&bar_from(100, &[("100.00", "150", Side::Sell)]));
    assert_eq!(cvd.divergence(), None, "no new high, no divergence");

    // A higher high, but CVD is below where it was at the last high.
    cvd.update(&bar_from(200, &[("102.00", "10", Side::Buy)]));
    assert_eq!(
        cvd.divergence(),
        Some(Divergence::Bearish),
        "higher high on lower CVD must flag"
    );
}

#[test]
fn cvd_flags_a_bullish_divergence() {
    let mut cvd = Cvd::new();
    cvd.update(&bar_from(0, &[("100.00", "100", Side::Sell)]));
    cvd.update(&bar_from(100, &[("101.00", "150", Side::Buy)]));
    // A lower low, but CVD is above where it was at the last low.
    cvd.update(&bar_from(200, &[("99.00", "10", Side::Sell)]));
    assert_eq!(cvd.divergence(), Some(Divergence::Bullish));
}

#[test]
fn cvd_divergence_does_not_latch() {
    let mut cvd = Cvd::new();
    cvd.update(&bar_from(0, &[("101.00", "100", Side::Buy)]));
    cvd.update(&bar_from(100, &[("100.00", "150", Side::Sell)]));
    cvd.update(&bar_from(200, &[("102.00", "10", Side::Buy)]));
    assert!(cvd.divergence().is_some());

    // The next uneventful bar must clear it, or the UI shows a stale flag.
    cvd.update(&bar_from(300, &[("101.00", "1", Side::Buy)]));
    assert_eq!(cvd.divergence(), None);
}

#[test]
fn cvd_reset_clears_everything() {
    let mut cvd = Cvd::new();
    cvd.update(&bar_from(0, &[("100.00", "5", Side::Buy)]));
    cvd.reset();
    assert_eq!(cvd.total(), Qty::ZERO);
    assert_eq!(cvd.bars(), 0);
    assert_eq!(cvd.value(), None);
}

// --- VWAP -----------------------------------------------------------------

#[test]
fn vwap_is_volume_weighted_not_a_simple_mean() {
    let mut vwap = Vwap::new();
    // 10 at 100, 90 at 200 => (1000 + 18000) / 100 = 190, not 150.
    let bar = bar_from(
        0,
        &[("100.00", "10", Side::Buy), ("200.00", "90", Side::Buy)],
    );
    let value = vwap.update(&bar).expect("volume present");
    assert_eq!(value.vwap, px("190.00"));
    assert_eq!(value.volume, qty("100"));
}

#[test]
fn vwap_deviation_is_zero_at_a_single_price() {
    let mut vwap = Vwap::new();
    let bar = bar_from(0, &[("100.00", "50", Side::Buy), ("100.00", "50", Side::Sell)]);
    let value = vwap.update(&bar).unwrap();
    assert_eq!(value.vwap, px("100.00"));
    assert_eq!(
        value.deviation,
        Price::ZERO,
        "one price cannot have dispersion"
    );
    assert_eq!(value.upper(2.0), px("100.00"));
}

#[test]
fn vwap_bands_straddle_the_average() {
    let mut vwap = Vwap::new();
    // Equal size at 90 and 110: mean 100, deviation 10.
    let bar = bar_from(0, &[("90.00", "50", Side::Buy), ("110.00", "50", Side::Buy)]);
    let value = vwap.update(&bar).unwrap();

    assert_eq!(value.vwap, px("100.00"));
    let deviation = value.deviation.to_f64();
    assert!(
        (deviation - 10.0).abs() < 0.01,
        "expected ~10, got {deviation}"
    );
    assert!(value.upper(1.0) > value.vwap);
    assert!(value.lower(1.0) < value.vwap);
    assert_eq!(
        value.upper(1.0).minor() - value.vwap.minor(),
        value.vwap.minor() - value.lower(1.0).minor(),
        "bands must be symmetric"
    );
}

#[test]
fn vwap_accumulates_across_bars_until_reset() {
    let mut vwap = Vwap::new();
    vwap.update(&bar_from(0, &[("100.00", "100", Side::Buy)]));
    let value = vwap
        .update(&bar_from(100, &[("200.00", "100", Side::Buy)]))
        .unwrap();
    assert_eq!(value.vwap, px("150.00"), "both bars must count");

    // Reset re-anchors: the next bar stands alone.
    vwap.reset();
    let value = vwap
        .update(&bar_from(200, &[("300.00", "10", Side::Buy)]))
        .unwrap();
    assert_eq!(value.vwap, px("300.00"));
}

#[test]
fn vwap_survives_session_scale_notional() {
    // Σ(price × volume) at crypto prices overflows i64 minor units quickly;
    // the accumulator must be wide enough for a real session.
    let mut vwap = Vwap::new();
    for _ in 0..5_000 {
        vwap.add(px("95000.00"), qty("10"));
    }
    let value = vwap.value().unwrap().unwrap();
    assert_eq!(value.vwap, px("95000.00"), "accumulator overflowed");
    assert_eq!(value.volume, qty("50000"));
}

#[test]
fn vwap_anchored_to_trades_reports_a_value() {
    // Regression: value() gated on bar count, so a VWAP fed through add()
    // held real data but reported nothing.
    let mut vwap = Vwap::new();
    assert!(!vwap.is_warm());
    assert_eq!(vwap.value(), Some(None), "no data yet");

    vwap.add(px("100.00"), qty("5"));
    assert!(vwap.is_warm());
    assert_eq!(vwap.value().unwrap().unwrap().vwap, px("100.00"));
}

// --- Session profile ------------------------------------------------------

#[test]
fn profile_accumulates_across_bars_and_finds_the_poc() {
    let mut profile = SessionProfile::new(tick());
    profile.update(&bar_from(
        0,
        &[("100.00", "10", Side::Buy), ("100.25", "5", Side::Buy)],
    ));
    profile.update(&bar_from(
        100,
        &[("100.25", "40", Side::Sell), ("100.50", "3", Side::Buy)],
    ));

    assert_eq!(profile.volume(), qty("58"));
    assert_eq!(profile.poc().unwrap(), px("100.25"), "5 + 40 is the heaviest");
    // Delta: +10 +5 -40 +3
    assert_eq!(profile.delta(), qty("-22"));
    assert!(profile.span().is_some());
}

#[test]
fn profile_value_area_encloses_the_requested_share() {
    let mut profile = SessionProfile::new(tick());
    for (price, size) in [
        ("99.50", "5"),
        ("99.75", "20"),
        ("100.00", "80"),
        ("100.25", "25"),
        ("100.50", "6"),
    ] {
        profile.update(&bar_from(0, &[(price, size, Side::Buy)]));
    }

    let va = profile.value_area().unwrap();
    assert_eq!(va.poc, px("100.00"));
    assert!(va.low <= va.poc && va.poc <= va.high);
    let target = (profile.volume().minor() as f64 * DEFAULT_VALUE_AREA) as i64;
    assert!(va.volume.minor() >= target);
}

#[test]
fn profile_reset_keeps_the_tick_size() {
    let mut profile = SessionProfile::new(tick());
    profile.update(&bar_from(0, &[("100.00", "5", Side::Buy)]));
    profile.reset();

    assert_eq!(profile.volume(), Qty::ZERO);
    assert_eq!(profile.value(), None);
    // Still usable at the original precision.
    profile.update(&bar_from(0, &[("100.25", "5", Side::Buy)]));
    assert_eq!(profile.poc().unwrap(), px("100.25"));
}

// --- TPO ------------------------------------------------------------------

#[test]
fn tpo_counts_brackets_not_volume() {
    // One bracket of enormous volume must not outweigh three quiet ones.
    let bracket = 60 * atas_core::time::NANOS_PER_SEC;
    let mut tpo = TpoProfile::new(tick(), bracket);

    // Bracket 0: huge volume at 100.00 only.
    tpo.update(&bar_from(0, &[("100.00", "10000", Side::Buy)]));
    // Brackets 1, 2, 3: tiny volume at 100.25.
    for i in 1..4 {
        tpo.update(&bar_from(i * 60_000, &[("100.25", "1", Side::Buy)]));
    }
    tpo.finish();

    assert_eq!(tpo.count_at(px("100.00")), 1);
    assert_eq!(tpo.count_at(px("100.25")), 3);
    assert_eq!(
        tpo.poc().unwrap(),
        px("100.25"),
        "TPO POC follows time, not volume"
    );
    assert_eq!(tpo.bracket_count(), 4);
}

#[test]
fn tpo_counts_a_level_once_per_bracket() {
    let bracket = 60 * atas_core::time::NANOS_PER_SEC;
    let mut tpo = TpoProfile::new(tick(), bracket);
    // Three bars inside one bracket, all touching the same level.
    for i in 0..3 {
        tpo.update(&bar_from(i * 1_000, &[("100.00", "1", Side::Buy)]));
    }
    tpo.finish();
    assert_eq!(tpo.count_at(px("100.00")), 1, "one bracket is one TPO");
}

#[test]
fn tpo_marks_every_level_a_bar_spanned() {
    let bracket = 60 * atas_core::time::NANOS_PER_SEC;
    let mut tpo = TpoProfile::new(tick(), bracket);
    // A bar from 100.00 to 100.75 covers four levels at a 0.25 tick.
    tpo.update(&bar_from(
        0,
        &[("100.00", "1", Side::Buy), ("100.75", "1", Side::Buy)],
    ));
    tpo.finish();

    for level in ["100.00", "100.25", "100.50", "100.75"] {
        assert_eq!(tpo.count_at(px(level)), 1, "level {level}");
    }
    assert_eq!(tpo.count_at(px("101.00")), 0);
    assert_eq!(tpo.rows().count(), 4);
}

// --- Scanners -------------------------------------------------------------

#[test]
fn big_trades_flags_only_prints_at_or_over_the_threshold() {
    let mut scanner = BigTrades::new(qty("100"), 8);
    let t = |q: &str| Trade::new(Ts::from_millis(0), px("100.00"), qty(q), Side::Buy, 0);

    assert!(scanner.update(&t("99")).is_none());
    assert!(scanner.update(&t("100")).is_some(), "the threshold is inclusive");
    assert!(scanner.update(&t("500")).is_some());
    assert_eq!(scanner.seen(), 2);
    assert_eq!(scanner.recent().count(), 2);
}

#[test]
fn big_trades_keeps_a_bounded_history() {
    let mut scanner = BigTrades::new(qty("1"), 3);
    for i in 0..10 {
        scanner.update(&Trade::new(
            Ts::from_millis(i),
            px("100.00"),
            qty("5"),
            Side::Buy,
            i as u64,
        ));
    }
    assert_eq!(scanner.seen(), 10);
    assert_eq!(scanner.recent().count(), 3, "capacity must bound memory");
    // The newest is kept, the oldest dropped.
    assert_eq!(scanner.recent().last().unwrap().ts, Ts::from_millis(9));
}

#[test]
fn cluster_search_with_no_criteria_matches_nothing() {
    // A scanner that fires on every bar is worse than none at all.
    let mut search = ClusterSearch::new(ClusterCriteria::new());
    let bar = bar_from(0, &[("100.00", "10000", Side::Buy)]);
    assert!(search.scan(&bar).is_none());
    assert_eq!(search.scanned(), 1);
    assert!(search.hits().is_empty());
}

#[test]
fn cluster_search_filters_on_volume_and_delta() {
    let mut search = ClusterSearch::new(ClusterCriteria::new().min_volume(qty("100")));
    assert!(search.scan(&bar_from(0, &[("100.00", "50", Side::Buy)])).is_none());
    assert!(search.scan(&bar_from(0, &[("100.00", "150", Side::Buy)])).is_some());

    let mut search = ClusterSearch::new(ClusterCriteria::new().min_abs_delta(qty("50")));
    // Balanced flow: big volume, small delta.
    let balanced = bar_from(
        0,
        &[("100.00", "100", Side::Buy), ("100.00", "90", Side::Sell)],
    );
    assert!(search.scan(&balanced).is_none());
    // One-sided flow clears it, and a negative delta counts too.
    let sold = bar_from(0, &[("100.00", "60", Side::Sell)]);
    assert!(search.scan(&sold).is_some());
}

#[test]
fn cluster_search_finds_a_heavy_level() {
    let criteria = ClusterCriteria::new().min_level_volume(qty("500"));
    let mut search = ClusterSearch::new(criteria);

    // Lots of volume, but spread thinly across levels.
    let spread = bar_from(
        0,
        &[
            ("100.00", "100", Side::Buy),
            ("100.25", "100", Side::Buy),
            ("100.50", "100", Side::Buy),
        ],
    );
    assert!(search.scan(&spread).is_none());

    let concentrated = bar_from(0, &[("100.25", "600", Side::Buy)]);
    let hit = search.scan(&concentrated).expect("should match");
    assert_eq!(hit.heaviest_level, Some((px("100.25"), qty("600"))));
}

#[test]
fn cluster_search_requires_a_real_imbalance_stack() {
    let criteria = ClusterCriteria::new()
        .min_imbalance_stack(3)
        .imbalance_ratio(3.0);
    let mut search = ClusterSearch::new(criteria);

    // Balanced ladder: no imbalances at all.
    let balanced = bar_from(
        0,
        &[
            ("100.00", "10", Side::Sell),
            ("100.00", "10", Side::Buy),
            ("100.25", "10", Side::Sell),
            ("100.25", "10", Side::Buy),
        ],
    );
    assert!(search.scan(&balanced).is_none());

    // Four levels where each ask is 10x the bid diagonally below it.
    let mut prints: Vec<(String, String, Side)> = Vec::new();
    for i in 0..5 {
        let price = format!("{:.2}", 100.0 + i as f64 * 0.25);
        prints.push((price.clone(), "2".into(), Side::Sell));
        prints.push((price, "20".into(), Side::Buy));
    }
    let refs: Vec<(&str, &str, Side)> = prints
        .iter()
        .map(|(p, q, s)| (p.as_str(), q.as_str(), *s))
        .collect();
    let stacked = bar_from(0, &refs);

    let hit = search.scan(&stacked).expect("stack should match");
    assert!(!hit.stacks.is_empty());
    assert!(hit.stacks[0].len() >= 3);
    assert_eq!(search.hits().len(), 1);
}

#[test]
fn speed_of_tape_measures_a_rolling_rate() {
    let second = atas_core::time::NANOS_PER_SEC;
    let mut speed = SpeedOfTape::new(second);

    // Ten trades inside one second.
    for i in 0..10 {
        speed.update(&Trade::new(
            Ts::from_millis(i * 100),
            px("100.00"),
            qty("1"),
            Side::Buy,
            i as u64,
        ));
    }
    assert_eq!(speed.count(), 10);
    assert!((speed.value().unwrap() - 10.0).abs() < 1e-9);

    // A trade ten seconds later empties the window behind it.
    speed.update(&Trade::new(
        Ts::from_secs(10),
        px("100.00"),
        qty("1"),
        Side::Buy,
        99,
    ));
    assert_eq!(speed.count(), 1, "stale trades must age out");
    assert!((speed.value().unwrap() - 1.0).abs() < 1e-9);
}

// --- Classical ------------------------------------------------------------

#[test]
fn sma_is_cold_until_its_window_fills() {
    let mut sma = Sma::new(3);
    let close = |p: &str, ms: i64| bar_from(ms, &[(p, "1", Side::Buy)]);

    assert_eq!(sma.update(&close("10.00", 0)), None);
    assert!(!sma.is_warm(), "one bar is not a 3-period average");
    assert_eq!(sma.update(&close("20.00", 1)), None);
    assert_eq!(sma.update(&close("30.00", 2)), Some(px("20.00")));
    assert!(sma.is_warm());

    // The window rolls: (20 + 30 + 40) / 3 = 30.
    assert_eq!(sma.update(&close("40.00", 3)), Some(px("30.00")));
}

#[test]
fn ema_seeds_on_the_first_close_and_tracks_price() {
    let mut ema = Ema::new(3);
    let close = |p: &str, ms: i64| bar_from(ms, &[(p, "1", Side::Buy)]);

    assert_eq!(ema.update(&close("10.00", 0)), Some(px("10.00")));
    assert!(!ema.is_warm(), "seeded is not warmed up");

    // alpha = 0.5 for period 3: 10 + 0.5 * (20 - 10) = 15.
    assert_eq!(ema.update(&close("20.00", 1)), Some(px("15.00")));
    ema.update(&close("20.00", 2));
    assert!(ema.is_warm());
}

#[test]
fn atr_uses_true_range_including_gaps() {
    let mut atr = Atr::new(2);
    // Bar 1: 100 to 110, range 10, no previous close.
    atr.update(&bar_from(
        0,
        &[("100.00", "1", Side::Buy), ("110.00", "1", Side::Buy)],
    ));
    // Bar 2 gaps up: low 130, high 140. Range is 10, but the gap from the
    // previous close of 110 makes the true range 30.
    let value = atr.update(&bar_from(
        1,
        &[("130.00", "1", Side::Buy), ("140.00", "1", Side::Buy)],
    ));
    let atr_value = value.expect("warm after 2 bars").to_f64();
    assert!(
        (atr_value - 20.0).abs() < 0.01,
        "expected (10 + 30) / 2 = 20, got {atr_value}"
    );
}

#[test]
fn rsi_is_bounded_and_pegs_on_an_unbroken_run() {
    let mut rsi = Rsi::new(3);
    let close = |p: &str, ms: i64| bar_from(ms, &[(p, "1", Side::Buy)]);

    // Nothing until there is a previous close to compare against.
    assert_eq!(rsi.update(&close("100.00", 0)), None);

    for (i, price) in ["101.00", "102.00", "103.00"].iter().enumerate() {
        rsi.update(&close(price, i as i64 + 1));
    }
    let value = rsi.value().unwrap().expect("warm");
    assert_eq!(value, 100.0, "no losses means RSI is 100 by definition");

    // Falling prices must pull it back inside the range.
    for (i, price) in ["95.00", "90.00", "85.00", "80.00"].iter().enumerate() {
        rsi.update(&close(price, i as i64 + 10));
    }
    let value = rsi.value().unwrap().unwrap();
    assert!((0.0..=100.0).contains(&value), "RSI escaped its range: {value}");
    assert!(value < 50.0, "a downtrend should read below 50, got {value}");
}
