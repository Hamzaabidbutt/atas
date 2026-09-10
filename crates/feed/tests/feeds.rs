//! Behavioural tests for the network-free feeds.

use atas_core::{Instrument, MarketEvent, Price, Qty, Side, Trade, Ts, Venue};
use atas_engine::{Aggregator, BarSpec};
use atas_feed::{drain, Feed, ReplayFeed, SyntheticFeed};
use atas_store::TickStore;
use tempfile::TempDir;

fn px(s: &str) -> Price {
    Price::parse(s).unwrap()
}
fn qty(s: &str) -> Qty {
    Qty::parse(s).unwrap()
}
fn instrument() -> Instrument {
    Instrument::spot("BTCUSDT", Venue::Binance, px("0.01"), qty("0.00001"))
}

fn trade(i: i64) -> Trade {
    Trade::new(
        Ts::from_millis(1_700_000_000_000 + i * 10),
        Price::from_minor(9_500_000_000_000 + i * 1_000_000),
        qty("1"),
        if i % 3 == 0 { Side::Sell } else { Side::Buy },
        i as u64,
    )
}

fn seeded_store(dir: &TempDir, count: i64) -> TickStore {
    let mut store = TickStore::open(dir.path(), &instrument()).unwrap();
    store
        .append_batch(&(0..count).map(trade).collect::<Vec<_>>())
        .unwrap();
    store.flush().unwrap();
    store
}

// --- Replay ---------------------------------------------------------------

#[test]
fn replays_stored_history_in_order() {
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir, 500);
    let mut feed = ReplayFeed::all(store);

    let events = drain(&mut feed).unwrap();
    assert_eq!(events.len(), 500, "every stored trade must be replayed");

    let mut previous = Ts::EPOCH;
    for (i, event) in events.iter().enumerate() {
        let MarketEvent::Trade(t) = event else {
            panic!("replay emits trades, got {event:?}");
        };
        assert_eq!(*t, trade(i as i64), "event {i}");
        assert!(t.ts >= previous, "replay came back out of order at {i}");
        previous = t.ts;
    }
    assert_eq!(feed.emitted(), 500);
    assert!(feed.is_finished());
}

#[test]
fn replay_respects_its_window() {
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir, 200);
    let mut feed = ReplayFeed::new(store, trade(50).ts, trade(60).ts);

    let events = drain(&mut feed).unwrap();
    assert_eq!(events.len(), 10, "[50, 60) is ten trades");

    let MarketEvent::Trade(first) = &events[0] else {
        panic!()
    };
    let MarketEvent::Trade(last) = &events[9] else {
        panic!()
    };
    assert_eq!(*first, trade(50));
    assert_eq!(*last, trade(59));
    assert_eq!(feed.window(), (trade(50).ts, trade(60).ts));
}

#[test]
fn replay_chunk_size_does_not_change_the_output() {
    // The window size is an I/O detail; varying it must not add, drop or
    // reorder a single event.
    let dir = TempDir::new().unwrap();
    let reference = {
        let store = TickStore::open(dir.path(), &instrument()).unwrap();
        let mut store = store;
        store
            .append_batch(&(0..1_000).map(trade).collect::<Vec<_>>())
            .unwrap();
        store.flush().unwrap();
        drain(&mut ReplayFeed::all(store)).unwrap()
    };
    assert_eq!(reference.len(), 1_000);

    for chunk in [1, 1_000, 1_000_000, 10_000_000_000] {
        let store = TickStore::open(dir.path(), &instrument()).unwrap();
        let mut feed = ReplayFeed::all(store).with_chunk_nanos(chunk);
        let events = drain(&mut feed).unwrap();
        assert_eq!(events, reference, "chunk size {chunk} changed the stream");
    }
}

#[test]
fn replay_skips_long_quiet_gaps_quickly() {
    // Regression: an empty window used to advance by exactly one chunk, so a
    // chunk small relative to the replay span looped effectively forever.
    // Real history has overnight gaps that hit the same path.
    let dir = TempDir::new().unwrap();
    let inst = instrument();
    let mut store = TickStore::open(dir.path(), &inst).unwrap();

    let early = Trade::new(Ts::from_secs(0), px("95000.00"), qty("1"), Side::Buy, 1);
    // Twelve hours later, at a one-nanosecond chunk: 4.3e13 empty windows if
    // the skip is linear.
    let late = Trade::new(Ts::from_secs(43_200), px("95001.00"), qty("2"), Side::Sell, 2);
    store.append_batch(&[early, late]).unwrap();
    store.flush().unwrap();

    let started = std::time::Instant::now();
    let mut feed = ReplayFeed::all(store).with_chunk_nanos(1);
    let events = drain(&mut feed).unwrap();

    assert_eq!(events.len(), 2);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "skipping a 12-hour gap took {:?}",
        started.elapsed()
    );
}

#[test]
fn replaying_an_empty_store_yields_nothing() {
    let dir = TempDir::new().unwrap();
    let store = TickStore::open(dir.path(), &instrument()).unwrap();
    let mut feed = ReplayFeed::all(store);

    assert!(drain(&mut feed).unwrap().is_empty());
    assert!(feed.is_finished());
    assert_eq!(feed.emitted(), 0);
}

#[test]
fn replay_progress_advances_to_completion() {
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir, 300);
    let mut feed = ReplayFeed::all(store).with_chunk_nanos(50_000_000);

    assert_eq!(feed.progress(), 0.0);
    feed.next_event().unwrap().expect("first event");
    let partway = feed.progress();
    assert!(partway > 0.0 && partway <= 1.0, "got {partway}");

    drain(&mut feed).unwrap();
    assert_eq!(feed.progress(), 1.0);
}

#[test]
fn an_inverted_replay_window_is_empty() {
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir, 100);
    let mut feed = ReplayFeed::new(store, trade(80).ts, trade(20).ts);
    assert!(drain(&mut feed).unwrap().is_empty());
    assert_eq!(feed.progress(), 1.0, "a degenerate window reads as done");
}

#[test]
fn replay_drives_the_aggregator_end_to_end() {
    // The point of replay: the same engine code runs off disk as off a socket.
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir, 1_000);
    let mut feed = ReplayFeed::all(store);
    let mut agg = Aggregator::new(&instrument(), BarSpec::Tick { count: 100 }).unwrap();

    let mut bars = 0;
    let mut volume = Qty::ZERO;
    while let Some(event) = feed.next_event().unwrap() {
        if let MarketEvent::Trade(t) = event {
            for bar in agg.on_trade(&t) {
                bars += 1;
                volume += bar.volume;
            }
        }
    }
    if let Some(bar) = agg.flush() {
        bars += 1;
        volume += bar.volume;
    }

    assert_eq!(bars, 10, "1000 trades at 100 per bar");
    assert_eq!(volume, qty("1000"));
    assert_eq!(agg.dropped_out_of_order(), 0);
    // Delta must survive the round trip through disk unchanged.
    let expected: Qty = (0..1_000).map(|i| trade(i).signed_qty()).sum();
    assert_eq!(agg.cum_delta(), expected);
}

// --- Synthetic ------------------------------------------------------------

#[test]
fn synthetic_is_reproducible_for_a_seed() {
    let make = || {
        let mut feed = SyntheticFeed::new(&instrument(), 42, px("95000.00")).with_limit(500);
        drain(&mut feed).unwrap()
    };
    assert_eq!(make(), make(), "same seed must give the same stream");
}

#[test]
fn different_seeds_diverge() {
    let mut a = SyntheticFeed::new(&instrument(), 1, px("95000.00")).with_limit(200);
    let mut b = SyntheticFeed::new(&instrument(), 2, px("95000.00")).with_limit(200);
    assert_ne!(drain(&mut a).unwrap(), drain(&mut b).unwrap());
}

#[test]
fn a_zero_seed_still_produces_a_varying_stream() {
    // Zero is a fixed point of xorshift; the constructor must substitute.
    let mut feed = SyntheticFeed::new(&instrument(), 0, px("95000.00")).with_limit(100);
    let events = drain(&mut feed).unwrap();
    assert_eq!(events.len(), 100);

    let prices: std::collections::HashSet<_> = events
        .iter()
        .filter_map(|e| match e {
            MarketEvent::Trade(t) => Some(t.price),
            _ => None,
        })
        .collect();
    assert!(prices.len() > 1, "a zero seed produced a frozen price");
}

#[test]
fn synthetic_respects_its_limit_then_stops() {
    let mut feed = SyntheticFeed::new(&instrument(), 7, px("95000.00")).with_limit(37);
    assert_eq!(drain(&mut feed).unwrap().len(), 37);
    assert_eq!(feed.emitted(), 37);
    assert!(feed.next_event().unwrap().is_none(), "stays exhausted");
}

#[test]
fn synthetic_timestamps_are_monotonic_and_on_tick() {
    let inst = instrument();
    let mut feed = SyntheticFeed::new(&inst, 99, px("95000.00")).with_limit(2_000);
    let events = drain(&mut feed).unwrap();

    let mut previous = Ts::EPOCH;
    let mut trades = 0;
    for event in &events {
        assert!(event.ts() >= previous, "timestamps went backwards");
        previous = event.ts();
        if let MarketEvent::Trade(t) = event {
            trades += 1;
            assert!(t.qty.is_positive(), "a trade had non-positive size");
            assert!(
                inst.is_on_tick(t.price),
                "generated an off-tick price {}",
                t.price
            );
        }
    }
    assert!(trades > 0);
}

#[test]
fn synthetic_emits_books_on_the_configured_cadence() {
    let mut feed = SyntheticFeed::new(&instrument(), 3, px("95000.00"))
        .with_book_every(10)
        .with_limit(100);
    let events = drain(&mut feed).unwrap();

    let books = events
        .iter()
        .filter(|e| matches!(e, MarketEvent::BookSnapshot { .. }))
        .count();
    assert_eq!(books, 10, "one book per ten events");

    // Books must be usable: sorted sides that do not cross.
    for event in &events {
        if let MarketEvent::BookSnapshot { bids, asks, .. } = event {
            let best_bid = bids.iter().map(|(p, _)| *p).max().unwrap();
            let best_ask = asks.iter().map(|(p, _)| *p).min().unwrap();
            assert!(best_bid < best_ask, "generated a crossed book");
            assert!(bids.iter().all(|(_, q)| q.is_positive()));
        }
    }
}

#[test]
fn book_output_can_be_disabled() {
    let mut feed = SyntheticFeed::new(&instrument(), 5, px("95000.00"))
        .with_book_every(0)
        .with_limit(50);
    let events = drain(&mut feed).unwrap();
    assert_eq!(events.len(), 50);
    assert!(events.iter().all(|e| e.is_trade()));
}

#[test]
fn synthetic_price_stays_near_its_anchor() {
    // The walk is mean-reverting; over a long run it must not drift away or
    // reach zero, which would make it useless for UI development.
    let anchor = px("95000.00");
    let mut feed = SyntheticFeed::new(&instrument(), 11, anchor)
        .with_book_every(0)
        .with_limit(100_000);
    let events = drain(&mut feed).unwrap();

    let mut low = Price::MAX;
    let mut high = Price::MIN;
    for event in &events {
        if let MarketEvent::Trade(t) = event {
            low = low.min(t.price);
            high = high.max(t.price);
            assert!(t.price.is_positive(), "price walked to or below zero");
        }
    }
    let drift = (high - anchor).max(anchor - low);
    assert!(
        drift < px("500.00"),
        "walked {drift} from the anchor over 100k trades"
    );
}

#[test]
fn synthetic_feeds_the_aggregator_without_dropped_trades() {
    let inst = instrument();
    let mut feed = SyntheticFeed::new(&inst, 21, px("95000.00")).with_limit(20_000);
    let mut agg = Aggregator::new(&inst, BarSpec::seconds(1)).unwrap();

    let mut bars = 0;
    while let Some(event) = feed.next_event().unwrap() {
        if let MarketEvent::Trade(t) = event {
            bars += agg.on_trade(&t).len();
        }
    }
    assert!(bars > 0, "20k synthetic trades produced no bars");
    assert_eq!(
        agg.dropped_out_of_order(),
        0,
        "the generator emitted out-of-order trades"
    );
}

#[test]
fn synthetic_output_can_be_stored_and_replayed_identically() {
    // Round trip: generate, persist, replay. Any encoding bug shows up here.
    let dir = TempDir::new().unwrap();
    let inst = instrument();

    let mut feed = SyntheticFeed::new(&inst, 77, px("95000.00"))
        .with_book_every(0)
        .with_limit(5_000);
    let generated: Vec<Trade> = drain(&mut feed)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            MarketEvent::Trade(t) => Some(t),
            _ => None,
        })
        .collect();

    let mut store = TickStore::open(dir.path(), &inst).unwrap();
    store.append_batch(&generated).unwrap();
    store.flush().unwrap();

    let replayed: Vec<Trade> = drain(&mut ReplayFeed::all(store))
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            MarketEvent::Trade(t) => Some(t),
            _ => None,
        })
        .collect();

    assert_eq!(replayed, generated, "storage round trip altered the stream");
}
