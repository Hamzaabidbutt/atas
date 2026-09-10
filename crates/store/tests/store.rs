//! Behavioural tests for the tick store, including durability and repair.

use std::fs::{self, OpenOptions};
use std::io::Write;

use atas_core::{Instrument, Price, Qty, Side, Trade, Ts, Venue};
use atas_store::{StoreError, TickStore, RECORD_SIZE};
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

/// Trade `i`, at a price that encodes its index so reads are identifiable.
fn trade(i: i64) -> Trade {
    Trade::new(
        Ts::from_millis(1_700_000_000_000 + i),
        Price::from_minor(100_000_000 + i),
        qty("1"),
        if i % 2 == 0 { Side::Buy } else { Side::Sell },
        i as u64,
    )
}

fn seeded(dir: &TempDir, count: i64) -> TickStore {
    let mut store = TickStore::open_dir(dir.path().join("ticks")).unwrap();
    let trades: Vec<_> = (0..count).map(trade).collect();
    store.append_batch(&trades).unwrap();
    store.flush().unwrap();
    store
}

#[test]
fn writes_and_reads_back_every_trade() {
    let dir = TempDir::new().unwrap();
    let mut store = seeded(&dir, 1_000);

    assert_eq!(store.len(), 1_000);
    assert!(!store.is_empty());
    assert_eq!(store.first_ts(), Some(trade(0).ts));
    assert_eq!(store.last_ts(), Some(trade(999).ts));

    let all = store.read_all().unwrap();
    assert_eq!(all.len(), 1_000);
    for (i, got) in all.iter().enumerate() {
        assert_eq!(*got, trade(i as i64), "record {i}");
    }
}

#[test]
fn an_empty_store_reads_as_empty() {
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks")).unwrap();

    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
    assert_eq!(store.first_ts(), None);
    assert_eq!(store.last_ts(), None);
    assert!(store.read_all().unwrap().is_empty());
    assert!(store
        .read(Ts::EPOCH, Ts::MAX)
        .unwrap()
        .is_empty());
}

#[test]
fn range_queries_are_half_open() {
    let dir = TempDir::new().unwrap();
    let mut store = seeded(&dir, 100);

    let got = store.read(trade(10).ts, trade(20).ts).unwrap();
    assert_eq!(got.len(), 10, "[10, 20) is ten records");
    assert_eq!(got.first().unwrap().ts, trade(10).ts);
    assert_eq!(got.last().unwrap().ts, trade(19).ts);
}

#[test]
fn range_queries_clamp_to_stored_history() {
    let dir = TempDir::new().unwrap();
    let mut store = seeded(&dir, 100);

    // Entirely before history.
    assert!(store.read(Ts::from_millis(0), trade(0).ts).unwrap().is_empty());
    // Entirely after history.
    assert!(store
        .read(trade(100).ts, trade(200).ts)
        .unwrap()
        .is_empty());
    // Straddling both ends returns everything.
    assert_eq!(
        store.read(Ts::from_millis(0), trade(1_000).ts).unwrap().len(),
        100
    );
    // Inverted and empty ranges yield nothing.
    assert!(store.read(trade(50).ts, trade(10).ts).unwrap().is_empty());
    assert!(store.read(trade(50).ts, trade(50).ts).unwrap().is_empty());
}

#[test]
fn rolls_segments_and_reads_across_them() {
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks"))
        .unwrap()
        .with_segment_records(100);

    for i in 0..1_000 {
        store.append(&trade(i)).unwrap();
    }
    store.flush().unwrap();

    assert_eq!(store.segment_count(), 10);
    assert_eq!(store.len(), 1_000);

    // A range spanning three segments must stitch them seamlessly.
    let got = store.read(trade(150).ts, trade(350).ts).unwrap();
    assert_eq!(got.len(), 200);
    assert_eq!(got[0], trade(150));
    assert_eq!(got[199], trade(349));
    assert!(got.windows(2).all(|w| w[0].ts <= w[1].ts));
}

#[test]
fn a_batch_larger_than_a_segment_is_split() {
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks"))
        .unwrap()
        .with_segment_records(10);

    let trades: Vec<_> = (0..95).map(trade).collect();
    store.append_batch(&trades).unwrap();
    store.flush().unwrap();

    assert_eq!(store.len(), 95);
    assert_eq!(store.segment_count(), 10, "9 full segments plus a partial");
    assert_eq!(store.read_all().unwrap(), trades);
}

#[test]
fn reopening_recovers_all_history() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ticks");

    {
        let mut store = TickStore::open_dir(&path).unwrap().with_segment_records(64);
        let trades: Vec<_> = (0..500).map(trade).collect();
        store.append_batch(&trades).unwrap();
        // Dropped without an explicit flush: the Drop impl must not lose data.
    }

    let mut reopened = TickStore::open_dir(&path).unwrap();
    assert_eq!(reopened.len(), 500);
    assert_eq!(reopened.first_ts(), Some(trade(0).ts));
    assert_eq!(reopened.last_ts(), Some(trade(499).ts));
    assert_eq!(reopened.read_all().unwrap().len(), 500);
}

#[test]
fn appends_continue_after_reopening() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ticks");

    {
        let mut store = TickStore::open_dir(&path).unwrap();
        store.append_batch(&(0..50).map(trade).collect::<Vec<_>>()).unwrap();
    }
    {
        let mut store = TickStore::open_dir(&path).unwrap();
        store.append_batch(&(50..100).map(trade).collect::<Vec<_>>()).unwrap();
        store.flush().unwrap();
        assert_eq!(store.len(), 100);
    }

    let mut store = TickStore::open_dir(&path).unwrap();
    let all = store.read_all().unwrap();
    assert_eq!(all.len(), 100);
    assert_eq!(all[99], trade(99));
}

#[test]
fn rejects_out_of_order_appends() {
    let dir = TempDir::new().unwrap();
    let mut store = seeded(&dir, 10);

    let err = store.append(&trade(5)).unwrap_err();
    assert!(
        matches!(err, StoreError::OutOfOrder { .. }),
        "expected OutOfOrder, got {err:?}"
    );
    assert_eq!(store.len(), 10, "a rejected append must not be written");
}

#[test]
fn a_bad_batch_is_rejected_whole() {
    let dir = TempDir::new().unwrap();
    let mut store = seeded(&dir, 10);

    // Valid, valid, then a step backwards. None of it may land.
    let batch = vec![trade(10), trade(11), trade(3)];
    assert!(store.append_batch(&batch).is_err());
    assert_eq!(store.len(), 10);
    assert_eq!(store.last_ts(), Some(trade(9).ts));
    assert_eq!(store.read_all().unwrap().len(), 10);
}

#[test]
fn equal_timestamps_are_accepted() {
    // Venues stamp a swept book with one millisecond across many prints.
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks")).unwrap();

    let same = Ts::from_millis(1_700_000_000_000);
    let batch: Vec<_> = (0..10)
        .map(|i| Trade::new(same, px("100"), qty("1"), Side::Buy, i))
        .collect();
    store.append_batch(&batch).unwrap();
    store.flush().unwrap();

    assert_eq!(store.len(), 10);
    assert_eq!(store.read(same, same + 1).unwrap().len(), 10);
}

#[test]
fn an_empty_batch_is_a_no_op() {
    let dir = TempDir::new().unwrap();
    let mut store = seeded(&dir, 5);
    store.append_batch(&[]).unwrap();
    assert_eq!(store.len(), 5);
}

#[test]
fn repairs_a_torn_final_record() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ticks");

    {
        let mut store = TickStore::open_dir(&path).unwrap();
        store.append_batch(&(0..20).map(trade).collect::<Vec<_>>()).unwrap();
        store.flush().unwrap();
    }

    // Simulate a process killed mid-write: append half a record.
    let segment = path.join("seg-000000.tick");
    let before = fs::metadata(&segment).unwrap().len();
    let mut file = OpenOptions::new().append(true).open(&segment).unwrap();
    file.write_all(&[0xAB; RECORD_SIZE / 2]).unwrap();
    file.flush().unwrap();
    drop(file);
    assert_ne!(fs::metadata(&segment).unwrap().len(), before);

    // Opening must truncate the torn tail and keep every whole record.
    let mut store = TickStore::open_dir(&path).unwrap();
    assert_eq!(store.len(), 20);
    assert_eq!(fs::metadata(&segment).unwrap().len(), before);
    assert_eq!(store.read_all().unwrap().len(), 20);

    // And the repaired store is still writable.
    store.append(&trade(100)).unwrap();
    store.flush().unwrap();
    assert_eq!(store.len(), 21);
}

#[test]
fn drops_an_empty_trailing_segment_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ticks");
    {
        let mut store = TickStore::open_dir(&path).unwrap();
        store.append_batch(&(0..5).map(trade).collect::<Vec<_>>()).unwrap();
        store.flush().unwrap();
    }
    // A crash right after rolling leaves a zero-length segment behind.
    fs::write(path.join("seg-000001.tick"), b"").unwrap();

    let store = TickStore::open_dir(&path).unwrap();
    assert_eq!(store.segment_count(), 1);
    assert!(!path.join("seg-000001.tick").exists());
}

#[test]
fn refuses_a_directory_that_is_not_a_store() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("not-a-store");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("readme.txt"), b"unrelated").unwrap();

    let err = TickStore::open_dir(&path).unwrap_err();
    assert!(
        matches!(err, StoreError::NotAStore(_)),
        "expected NotAStore, got {err:?}"
    );
}

#[test]
fn refuses_an_incompatible_format_version() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ticks");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join(".atas-tickstore"), b"999").unwrap();

    let err = TickStore::open_dir(&path).unwrap_err();
    assert!(
        matches!(
            err,
            StoreError::UnsupportedVersion {
                found: 999,
                expected: 1
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn scan_sees_buffered_records_without_an_explicit_flush() {
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks")).unwrap();
    store.append_batch(&(0..10).map(trade).collect::<Vec<_>>()).unwrap();

    // No flush() call: scan must flush for itself or silently miss records.
    assert_eq!(store.read_all().unwrap().len(), 10);
}

#[test]
fn instrument_history_is_namespaced_by_venue_and_symbol() {
    let dir = TempDir::new().unwrap();
    let inst = instrument();
    let mut store = TickStore::open(dir.path(), &inst).unwrap();
    store.append(&trade(0)).unwrap();
    store.flush().unwrap();

    let expected = dir.path().join("binance").join("BTCUSDT");
    assert_eq!(store.root(), expected);
    assert!(expected.join("seg-000000.tick").exists());

    // A different venue with the same symbol is a separate store.
    let other = Instrument::spot("BTCUSDT", Venue::Bybit, px("0.01"), qty("0.00001"));
    let other_store = TickStore::open(dir.path(), &other).unwrap();
    assert!(other_store.is_empty());
    assert_ne!(other_store.root(), store.root());
}

#[test]
fn binary_search_lands_exactly_on_boundaries() {
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks"))
        .unwrap()
        .with_segment_records(37); // deliberately not a round number

    let trades: Vec<_> = (0..500).map(trade).collect();
    store.append_batch(&trades).unwrap();
    store.flush().unwrap();

    // Probe every offset: an off-by-one in the search shows up immediately.
    for i in 0..500i64 {
        let got = store.read(trade(i).ts, trade(i).ts + 1).unwrap();
        assert_eq!(got.len(), 1, "at {i}");
        assert_eq!(got[0], trade(i), "at {i}");
    }
}

#[test]
fn survives_a_large_history_with_correct_ordering() {
    let dir = TempDir::new().unwrap();
    let mut store = TickStore::open_dir(dir.path().join("ticks"))
        .unwrap()
        .with_segment_records(10_000);

    let trades: Vec<_> = (0..250_000).map(trade).collect();
    store.append_batch(&trades).unwrap();
    store.sync().unwrap();

    assert_eq!(store.len(), 250_000);
    assert_eq!(store.segment_count(), 25);

    let mut seen = 0u64;
    let mut previous = Ts::EPOCH;
    store
        .scan(Ts::EPOCH, Ts::MAX, |t| {
            assert!(t.ts >= previous, "history came back out of order");
            previous = t.ts;
            seen += 1;
        })
        .unwrap();
    assert_eq!(seen, 250_000);

    // A narrow window deep in history must not read the whole file.
    let window = store.read(trade(199_000).ts, trade(199_010).ts).unwrap();
    assert_eq!(window.len(), 10);
    assert_eq!(window[0], trade(199_000));
}
