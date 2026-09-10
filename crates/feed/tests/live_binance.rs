//! Live Binance connectivity.
//!
//! Every other test in this crate runs offline against recorded payloads. This
//! one opens a real socket to a real venue, and it is the only way to find out
//! whether the transport actually works — the wire parsers being correct says
//! nothing about whether the handshake, the subscription or the sequencing do.
//!
//! Marked `#[ignore]` so it never runs in an ordinary `cargo test`: it needs
//! network access, it depends on a third party being up, and a developer
//! offline should not see red. Run it deliberately:
//!
//! ```sh
//! cargo test -p atas-feed --features live -- --ignored --nocapture
//! ```
//!
//! These connect to Binance's `.vision` market-data hosts rather than the
//! trading ones. The trading hosts answer 451 Unavailable For Legal Reasons
//! from several jurisdictions — including the United States, where most CI
//! networks live — and that applies to the websocket, not just REST. The
//! market-data hosts carry the same public data with no trading capability,
//! which is the correct endpoint for this client anyway since it never places
//! an order on Binance.

#![cfg(feature = "live")]

use std::time::{Duration, Instant};

use atas_core::{Instrument, MarketEvent, Price, Qty, Venue};
use atas_engine::{Aggregator, BarSpec};
use atas_feed::live::{binance, LiveOptions};
use atas_feed::Feed;

fn instrument() -> Instrument {
    Instrument::spot(
        "BTCUSDT",
        Venue::Binance,
        Price::parse("0.01").unwrap(),
        Qty::parse("0.00001").unwrap(),
    )
}

/// What a connection attempt observed.
#[derive(Debug, Default)]
struct Observed {
    trades: usize,
    snapshots: usize,
    deltas: usize,
    connected: bool,
    disconnects: Vec<String>,
    first_price: Option<Price>,
    last_price: Option<Price>,
}

/// Drain a live feed for `budget`, recording what arrived.
fn observe(feed: &mut impl Feed, budget: Duration) -> Observed {
    let started = Instant::now();
    let mut seen = Observed::default();

    while started.elapsed() < budget {
        match feed.next_event() {
            Ok(Some(MarketEvent::Trade(trade))) => {
                seen.trades += 1;
                seen.first_price.get_or_insert(trade.price);
                seen.last_price = Some(trade.price);
            }
            Ok(Some(MarketEvent::BookSnapshot { bids, asks, .. })) => {
                seen.snapshots += 1;
                assert!(
                    !bids.is_empty() && !asks.is_empty(),
                    "a snapshot arrived with an empty side"
                );
            }
            Ok(Some(MarketEvent::BookDelta { .. })) => seen.deltas += 1,
            Ok(Some(MarketEvent::Status {
                connected, detail, ..
            })) => {
                if connected {
                    seen.connected = true;
                } else {
                    seen.disconnects.push(detail);
                }
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("feed error: {e}"),
        }
    }
    seen
}

#[test]
#[ignore = "requires network access to Binance"]
fn connects_and_receives_real_trades() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let inst = instrument();
    let mut feed = binance::connect(&inst, LiveOptions::default(), runtime.handle());

    let seen = observe(&mut feed, Duration::from_secs(30));
    println!("observed: {seen:#?}");

    assert!(
        seen.connected,
        "never reported a connection; disconnects: {:?}",
        seen.disconnects
    );
    assert!(
        seen.trades > 0,
        "no trades in 30s on BTCUSDT, which is implausible unless the \
         stream is not actually delivering: {seen:#?}"
    );

    // Sanity-check the prices rather than merely counting messages: a parser
    // that returns zeros would otherwise pass.
    let first = seen.first_price.expect("a trade was counted");
    assert!(first.is_positive(), "traded at a non-positive price: {first}");
    assert!(
        first > Price::from_units(1_000) && first < Price::from_units(10_000_000),
        "BTCUSDT printed at {first}, which is not a plausible price"
    );

    if seen.snapshots == 0 {
        println!(
            "NOTE: no REST depth snapshot, though trades arrived. The depth \
             handshake is the part that did not complete."
        );
    } else {
        assert!(
            seen.deltas > 0,
            "a snapshot arrived but no depth updates followed it"
        );
    }
}

#[test]
#[ignore = "requires network access to Binance"]
fn live_trades_drive_the_aggregator() {
    // The point of the whole transport: real prints producing real footprints.
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let inst = instrument();
    let mut feed = binance::connect(&inst, LiveOptions::default(), runtime.handle());
    let mut agg = Aggregator::new(&inst, BarSpec::Tick { count: 25 }).unwrap();

    let started = Instant::now();
    let mut bars = 0usize;
    let mut volume = Qty::ZERO;

    while started.elapsed() < Duration::from_secs(45) && bars < 2 {
        match feed.next_event() {
            Ok(Some(MarketEvent::Trade(trade))) => {
                for bar in agg.on_trade(&trade) {
                    bars += 1;
                    volume += bar.volume;

                    assert!(bar.low <= bar.high, "bar low above high");
                    assert_eq!(
                        bar.volume,
                        bar.clusters.total_volume(),
                        "bar volume disagrees with its ladder"
                    );
                    assert!(bar.trades > 0);
                    println!(
                        "bar {}: {} trades, vol {}, delta {}, poc {:?}",
                        bars,
                        bar.trades,
                        bar.volume,
                        bar.delta(),
                        bar.poc()
                    );
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("feed error: {e}"),
        }
    }

    assert!(bars > 0, "no bars formed from live data in 45s");
    assert!(volume.is_positive(), "bars formed but carried no volume");
    assert_eq!(
        agg.dropped_out_of_order(),
        0,
        "the adapter delivered trades out of order, which corrupts history"
    );
}

#[test]
#[ignore = "requires network access to Binance"]
fn aggressor_sides_are_not_all_one_way() {
    // The single highest-consequence bug in this integration is inverting the
    // maker flag. That produces a plausible-looking chart with every delta
    // backwards. Real BTCUSDT flow is never one-sided for long, so an
    // all-buy or all-sell run over hundreds of prints means the flag is being
    // read as a constant.
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let inst = instrument();
    let mut feed = binance::connect(&inst, LiveOptions::default(), runtime.handle());

    let started = Instant::now();
    let mut buys = 0usize;
    let mut sells = 0usize;

    while started.elapsed() < Duration::from_secs(30) && buys + sells < 300 {
        match feed.next_event() {
            Ok(Some(MarketEvent::Trade(trade))) => match trade.aggressor {
                atas_core::Side::Buy => buys += 1,
                atas_core::Side::Sell => sells += 1,
            },
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("feed error: {e}"),
        }
    }

    println!("aggressor split: {buys} buys, {sells} sells");
    assert!(buys + sells > 20, "too few trades to judge: {buys}/{sells}");
    assert!(
        buys > 0 && sells > 0,
        "every print came back as the same side ({buys} buys, {sells} sells), \
         which means the maker flag is not being read"
    );
}
