//! Session tests: the whole platform driven end to end without a window.

use atas_app::{AppEvent, Session, SessionConfig, PRICE_SCALE};
use atas_core::{Instrument, MarketEvent, Price, Qty, Side, Trade, Ts, Venue};
use atas_engine::BarSpec;
use atas_feed::{Feed, SyntheticFeed};
use atas_indicators::ClusterCriteria;
use atas_trading::TradingConfig;

fn px(s: &str) -> Price {
    Price::parse(s).unwrap()
}
fn qty(s: &str) -> Qty {
    Qty::parse(s).unwrap()
}
fn instrument() -> Instrument {
    Instrument::spot("TEST", Venue::Sim, px("0.25"), qty("1"))
}

fn config() -> SessionConfig {
    SessionConfig {
        bar_spec: BarSpec::Tick { count: 3 },
        big_trade_threshold: qty("50"),
        trading: TradingConfig {
            commission_rate: 0.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn session() -> Session {
    Session::new(instrument(), config()).unwrap()
}

fn trade_event(price: &str, q: &str, side: Side, ms: i64) -> MarketEvent {
    MarketEvent::Trade(Trade::new(
        Ts::from_millis(ms),
        px(price),
        qty(q),
        side,
        ms as u64,
    ))
}

fn book_event(ms: i64) -> MarketEvent {
    MarketEvent::BookSnapshot {
        ts: Ts::from_millis(ms),
        bids: vec![(px("99.75"), qty("10")), (px("99.50"), qty("20"))],
        asks: vec![(px("100.00"), qty("10")), (px("100.25"), qty("20"))],
        sequence: ms as u64,
    }
}

#[test]
fn a_trade_produces_tape_and_a_forming_bar() {
    let mut s = session();
    let events = s.on_market_event(&trade_event("100.00", "5", Side::Buy, 1));

    assert!(events.iter().any(|e| matches!(e, AppEvent::TapeRow(_))));
    assert!(events.iter().any(|e| matches!(e, AppEvent::BarUpdated(_))));
    assert!(!events.iter().any(|e| matches!(e, AppEvent::BarClosed(_))));
    assert_eq!(s.forming_bar().unwrap().trades, 1);
}

#[test]
fn a_closing_bar_emits_bar_closed_and_advances_history() {
    let mut s = session(); // 3 trades per bar
    s.on_market_event(&trade_event("100.00", "5", Side::Buy, 1));
    s.on_market_event(&trade_event("100.25", "5", Side::Buy, 2));
    let events = s.on_market_event(&trade_event("100.50", "5", Side::Sell, 3));

    let closed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::BarClosed(bar) => Some(bar),
            _ => None,
        })
        .collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].trades, 3);
    assert!(closed[0].closed);
    assert_eq!(closed[0].volume, qty("15").minor());
    assert_eq!(s.bars().count(), 1);
}

#[test]
fn bar_history_is_bounded() {
    let mut s = Session::new(
        instrument(),
        SessionConfig {
            bar_spec: BarSpec::Tick { count: 1 },
            bar_history: 5,
            ..config()
        },
    )
    .unwrap();

    for i in 0..50 {
        s.on_market_event(&trade_event("100.00", "1", Side::Buy, i));
    }
    assert_eq!(s.bars().count(), 5, "history must not grow without bound");
}

#[test]
fn closed_bars_carry_their_footprint_and_imbalances() {
    let mut s = Session::new(
        instrument(),
        SessionConfig {
            bar_spec: BarSpec::Tick { count: 2 },
            ..config()
        },
    )
    .unwrap();

    // 10 on the bid at 100.00, 40 on the ask at 100.25: a diagonal buy
    // imbalance of 4.0, which clears the default ratio of 3.0.
    s.on_market_event(&trade_event("100.00", "10", Side::Sell, 1));
    let events = s.on_market_event(&trade_event("100.25", "40", Side::Buy, 2));

    let AppEvent::BarClosed(bar) = events
        .iter()
        .find(|e| matches!(e, AppEvent::BarClosed(_)))
        .expect("a bar should have closed")
    else {
        unreachable!()
    };

    assert_eq!(bar.clusters.len(), 2);
    assert_eq!(bar.poc, Some(px("100.25").minor()));
    assert_eq!(bar.imbalances.len(), 1);
    assert_eq!(bar.imbalances[0].price, px("100.25").minor());
    assert_eq!(bar.imbalances[0].side, "buy");
}

#[test]
fn a_book_snapshot_updates_depth() {
    let mut s = session();
    let events = s.on_market_event(&book_event(1));

    let AppEvent::BookUpdated(book) = events
        .iter()
        .find(|e| matches!(e, AppEvent::BookUpdated(_)))
        .expect("book event")
    else {
        unreachable!()
    };
    assert_eq!(book.bids.len(), 2);
    assert_eq!(book.asks.len(), 2);
    assert_eq!(book.spread, Some(px("0.25").minor()));
    assert!(!book.crossed);
}

#[test]
fn a_stale_book_delta_is_dropped_silently() {
    // REST and websocket overlap during startup; a stale sequence is normal,
    // not an error worth surfacing to the trader.
    let mut s = session();
    s.on_market_event(&book_event(10)); // sequence 10

    let stale = MarketEvent::BookDelta {
        ts: Ts::from_millis(11),
        bids: vec![(px("99.75"), qty("999"))],
        asks: vec![],
        sequence: 5,
    };
    let events = s.on_market_event(&stale);
    assert!(
        !events.iter().any(|e| matches!(e, AppEvent::BookUpdated(_))),
        "a rejected delta must not publish a book"
    );
    assert_eq!(s.book().qty_at(atas_core::BookSide::Bid, px("99.75")), qty("10"));
}

#[test]
fn a_disconnect_clears_the_book() {
    // Stale depth shown as live is worse than no depth at all.
    let mut s = session();
    s.on_market_event(&book_event(1));
    assert!(s.book().is_initialised());

    let events = s.on_market_event(&MarketEvent::Status {
        ts: Ts::from_millis(2),
        connected: false,
        detail: "socket closed".into(),
    });

    assert!(events
        .iter()
        .any(|e| matches!(e, AppEvent::ConnectionChanged { connected: false, .. })));
    assert!(!s.is_connected());
    assert!(!s.book().is_initialised());
    assert!(s.snapshot().book.bids.is_empty());
}

#[test]
fn big_trades_are_flagged_above_the_threshold() {
    let mut s = session(); // threshold 50
    let quiet = s.on_market_event(&trade_event("100.00", "10", Side::Buy, 1));
    assert!(!quiet.iter().any(|e| matches!(e, AppEvent::BigTrade { .. })));

    let loud = s.on_market_event(&trade_event("100.00", "500", Side::Buy, 2));
    let flagged = loud
        .iter()
        .find(|e| matches!(e, AppEvent::BigTrade { .. }))
        .expect("500 is over the threshold");
    let AppEvent::BigTrade { qty: q, side, .. } = flagged else {
        unreachable!()
    };
    assert_eq!(*q, qty("500").minor());
    assert_eq!(*side, "buy");
}

#[test]
fn the_scanner_fires_only_on_configured_criteria() {
    let mut s = Session::new(
        instrument(),
        SessionConfig {
            bar_spec: BarSpec::Tick { count: 1 },
            scanner: ClusterCriteria::new().min_volume(qty("100")),
            ..config()
        },
    )
    .unwrap();

    let small = s.on_market_event(&trade_event("100.00", "10", Side::Buy, 1));
    assert!(!small.iter().any(|e| matches!(e, AppEvent::ScannerHit { .. })));

    let large = s.on_market_event(&trade_event("100.00", "500", Side::Buy, 2));
    assert!(large.iter().any(|e| matches!(e, AppEvent::ScannerHit { .. })));
    assert_eq!(s.scanner_hits().len(), 1);
}

#[test]
fn orders_fill_against_the_session_book() {
    let mut s = session();
    s.on_market_event(&book_event(1));
    s.on_market_event(&trade_event("100.00", "1", Side::Buy, 2));

    let outcome = s.buy_market(qty("5")).unwrap();
    assert_eq!(outcome.fills.len(), 1);
    assert_eq!(outcome.fills[0].price, px("100.00"));
    assert_eq!(s.engine().position().qty, qty("5"));
}

#[test]
fn a_fill_from_a_trade_emits_fill_and_position_events() {
    let mut s = session();
    s.on_market_event(&book_event(1));
    s.on_market_event(&trade_event("100.00", "1", Side::Buy, 2));

    // Rest a buy limit below the market, then trade through it.
    s.place_limit(Side::Buy, qty("5"), px("99.00")).unwrap();
    let events = s.on_market_event(&trade_event("98.75", "10", Side::Sell, 3));

    assert!(events.iter().any(|e| matches!(e, AppEvent::Filled(_))));
    assert!(events
        .iter()
        .any(|e| matches!(e, AppEvent::PositionChanged(_))));
    assert_eq!(s.engine().position().qty, qty("5"));
}

#[test]
fn flatten_closes_out_through_the_session() {
    let mut s = session();
    s.on_market_event(&book_event(1));
    s.on_market_event(&trade_event("100.00", "1", Side::Buy, 2));

    s.buy_market(qty("5")).unwrap();
    s.place_limit(Side::Buy, qty("5"), px("90.00")).unwrap();
    assert_eq!(s.engine().working_orders().count(), 1);

    let fills = s.flatten().unwrap();
    assert!(!fills.is_empty());
    assert!(s.engine().position().is_flat());
    assert_eq!(s.engine().working_orders().count(), 0);
}

#[test]
fn a_rejected_order_leaves_the_session_untouched() {
    let mut s = session();
    // No book yet, so a market order cannot be priced.
    assert!(s.buy_market(qty("5")).is_err());
    assert!(s.engine().position().is_flat());
    assert!(s.engine().fills().is_empty());
}

// --- Snapshot -------------------------------------------------------------

#[test]
fn a_snapshot_describes_a_full_frame() {
    let mut s = session();
    s.on_market_event(&book_event(1));
    for i in 0..7 {
        s.on_market_event(&trade_event("100.00", "5", Side::Buy, i + 2));
    }

    let snap = s.snapshot();
    assert_eq!(snap.instrument, "sim:TEST");
    assert_eq!(snap.scale, PRICE_SCALE);
    assert_eq!(snap.tick_size, px("0.25").minor());
    assert_eq!(snap.price_decimals, 2);
    // Two closed bars at 3 trades each, plus the forming one.
    assert_eq!(snap.bars.len(), 3);
    assert!(!snap.bars.last().unwrap().closed, "the last bar is forming");
    assert_eq!(snap.tape.len(), 7);
    assert!(snap.last_price.is_some());
    assert_eq!(snap.book.bids.len(), 2);
}

#[test]
fn snapshot_prices_survive_the_javascript_number_boundary() {
    // Minor units must stay inside 2^53 so a JS number holds them exactly.
    let mut s = session();
    s.on_market_event(&trade_event("99999.99", "1", Side::Buy, 1));

    let snap = s.snapshot();
    let bar = snap.bars.last().unwrap();
    assert_eq!(bar.close, px("99999.99").minor());
    assert!(
        (bar.close as f64) < 9_007_199_254_740_992.0,
        "price {} would lose precision as a JS number",
        bar.close
    );
    // And it survives an actual JSON round trip through f64.
    let json = serde_json::to_string(&snap).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let close = parsed["bars"].as_array().unwrap().last().unwrap()["close"]
        .as_i64()
        .unwrap();
    assert_eq!(close, px("99999.99").minor());
}

#[test]
fn events_and_snapshots_serialise_to_json() {
    let mut s = session();
    s.on_market_event(&book_event(1));
    let events: Vec<AppEvent> = s
        .on_market_event(&trade_event("100.00", "500", Side::Buy, 2))
        .to_vec();

    let json = serde_json::to_string(&events).unwrap();
    // The tag makes each variant discriminable from TypeScript.
    assert!(json.contains("\"type\":\"tape_row\""), "{json}");
    assert!(json.contains("\"type\":\"big_trade\""), "{json}");
    serde_json::to_string(&s.snapshot()).unwrap();
}

// --- Configuration --------------------------------------------------------

#[test]
fn changing_the_bar_spec_discards_incomparable_history() {
    let mut s = session();
    for i in 0..7 {
        s.on_market_event(&trade_event("100.00", "5", Side::Buy, i));
    }
    assert!(s.bars().count() > 0);

    s.set_bar_spec(BarSpec::Volume {
        threshold: qty("100"),
    })
    .unwrap();
    assert_eq!(
        s.bars().count(),
        0,
        "bars built under the old rule cannot be reinterpreted under the new one"
    );
    assert!(s.forming_bar().is_none());

    // And the new rule takes effect.
    s.on_market_event(&trade_event("100.00", "150", Side::Buy, 100));
    assert_eq!(s.bars().count(), 1);
}

#[test]
fn an_invalid_bar_spec_is_rejected_without_disturbing_state() {
    let mut s = session();
    for i in 0..3 {
        s.on_market_event(&trade_event("100.00", "5", Side::Buy, i));
    }
    let before = s.bars().count();

    assert!(s.set_bar_spec(BarSpec::Tick { count: 0 }).is_err());
    assert_eq!(s.bars().count(), before, "a rejected change must change nothing");
}

#[test]
fn resetting_a_session_keeps_the_position() {
    // A session boundary is not a reason to abandon a live trade.
    let mut s = session();
    s.on_market_event(&book_event(1));
    s.on_market_event(&trade_event("100.00", "1", Side::Buy, 2));
    s.buy_market(qty("5")).unwrap();

    s.reset_session();
    assert_eq!(s.bars().count(), 0);
    assert!(s.snapshot().tape.is_empty());
    assert_eq!(
        s.engine().position().qty,
        qty("5"),
        "the position must survive a session reset"
    );
}

// --- End to end -----------------------------------------------------------

#[test]
fn a_synthetic_feed_drives_the_whole_session() {
    let inst = instrument();
    let mut s = Session::new(
        inst.clone(),
        SessionConfig {
            bar_spec: BarSpec::Tick { count: 50 },
            scanner: ClusterCriteria::new().min_imbalance_stack(3),
            ..config()
        },
    )
    .unwrap();

    let mut feed = SyntheticFeed::new(&inst, 4242, px("100.00")).with_limit(10_000);
    let mut tape_rows = 0u64;
    let mut closed_bars = 0u64;

    while let Some(event) = feed.next_event().unwrap() {
        for app_event in s.on_market_event(&event) {
            match app_event {
                AppEvent::TapeRow(_) => tape_rows += 1,
                AppEvent::BarClosed(_) => closed_bars += 1,
                _ => {}
            }
        }
    }

    assert!(tape_rows > 0, "no trades reached the tape");
    assert!(closed_bars > 0, "no bars closed");

    let snap = s.snapshot();
    assert!(!snap.bars.is_empty());
    assert!(snap.indicators.vwap.is_some(), "VWAP should be warm");
    assert!(snap.indicators.session_poc.is_some());
    // Every published bar must be internally consistent.
    for bar in &snap.bars {
        assert!(bar.low <= bar.high);
        let ladder: i64 = bar.clusters.iter().map(|c| c.bid + c.ask).sum();
        assert_eq!(ladder, bar.volume, "ladder disagrees with bar volume");
    }
    serde_json::to_string(&snap).unwrap();
}
