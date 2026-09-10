//! Bybit v5 wire format.
//!
//! Unlike Binance, Bybit reports the taker side directly in `S`, so no
//! inversion is needed — which is exactly why the two adapters must not share
//! a "side" helper. Bybit also distinguishes snapshot from delta with a `type`
//! field on the message rather than by endpoint.

use atas_core::{MarketEvent, Side, Trade, Ts};
use serde_json::Value;

use crate::error::FeedError;
use crate::json::{int_field, level_array, price_field, qty_field, str_field, uint_field};

/// Parse Bybit's taker-side field.
///
/// `S` is the side of the **taker**, so it maps straight through with no
/// inversion. Compare with Binance, where the equivalent field is inverted.
pub fn aggressor_from_taker_side(s: &str) -> Result<Side, FeedError> {
    match s {
        "Buy" => Ok(Side::Buy),
        "Sell" => Ok(Side::Sell),
        other => Err(FeedError::BadField {
            field: "S",
            context: "bybit publicTrade",
            value: other.to_string(),
        }),
    }
}

/// Parse any Bybit v5 public message into normalised events.
///
/// A single message can carry many trades, so this returns a vector.
pub fn parse_message(raw: &str) -> Result<Vec<MarketEvent>, FeedError> {
    let value: Value = serde_json::from_str(raw)?;
    parse_value(&value)
}

/// Parse an already-decoded Bybit message.
pub fn parse_value(value: &Value) -> Result<Vec<MarketEvent>, FeedError> {
    // Subscription acknowledgements have no topic and carry no market data.
    let Some(topic) = value.get("topic").and_then(Value::as_str) else {
        return Err(FeedError::Unhandled("no topic".to_string()));
    };

    if topic.starts_with("publicTrade") {
        return parse_trades(value).map(|ts| ts.into_iter().map(MarketEvent::Trade).collect());
    }
    if topic.starts_with("orderbook") {
        return parse_orderbook(value).map(|e| vec![e]);
    }
    Err(FeedError::Unhandled(topic.to_string()))
}

/// Parse a `publicTrade` message's `data` array.
pub fn parse_trades(value: &Value) -> Result<Vec<Trade>, FeedError> {
    const CTX: &str = "bybit publicTrade";
    let data = value.get("data").and_then(Value::as_array).ok_or({
        FeedError::MissingField {
            field: "data",
            context: CTX,
        }
    })?;

    data.iter()
        .map(|entry| {
            Ok(Trade {
                ts: Ts::from_millis(int_field(entry, "T", CTX)?),
                price: price_field(entry, "p", CTX)?,
                qty: qty_field(entry, "v", CTX)?,
                aggressor: aggressor_from_taker_side(str_field(entry, "S", CTX)?)?,
                // Bybit trade ids are UUIDs, which do not fit a u64. The store
                // keeps zero rather than a truncated id that could collide;
                // dedup for this venue keys on (ts, price, qty) instead.
                id: 0,
            })
        })
        .collect()
}

/// Parse an `orderbook.N.SYMBOL` message.
///
/// The message's `type` selects snapshot or delta. Bybit sends a snapshot on
/// subscribe and after any internal gap, so honouring this field is what keeps
/// the local book from drifting.
pub fn parse_orderbook(value: &Value) -> Result<MarketEvent, FeedError> {
    const CTX: &str = "bybit orderbook";

    let kind = str_field(value, "type", CTX)?;
    let ts = Ts::from_millis(int_field(value, "ts", CTX)?);
    let data = value.get("data").ok_or(FeedError::MissingField {
        field: "data",
        context: CTX,
    })?;

    let bids = level_array(data, "b", CTX)?;
    let asks = level_array(data, "a", CTX)?;
    let sequence = uint_field(data, "u", CTX)?;

    match kind {
        "snapshot" => Ok(MarketEvent::BookSnapshot {
            ts,
            bids,
            asks,
            sequence,
        }),
        "delta" => Ok(MarketEvent::BookDelta {
            ts,
            bids,
            asks,
            sequence,
        }),
        other => Err(FeedError::BadField {
            field: "type",
            context: CTX,
            value: other.to_string(),
        }),
    }
}

/// Topic name for an instrument's public trades.
pub fn trade_topic(symbol: &str) -> String {
    format!("publicTrade.{}", symbol.to_uppercase())
}

/// Topic name for an instrument's order book at a given depth.
pub fn orderbook_topic(symbol: &str, depth: u32) -> String {
    format!("orderbook.{depth}.{}", symbol.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atas_core::{Price, Qty};

    const TRADES: &str = r#"{
        "topic":"publicTrade.BTCUSDT","type":"snapshot","ts":1672304486868,
        "data":[
            {"T":1672304486865,"s":"BTCUSDT","S":"Buy","v":"0.001","p":"16578.50",
             "L":"PlusTick","i":"20f43950-d8dd-5b31-9112-a178eb6023af","BT":false},
            {"T":1672304486866,"s":"BTCUSDT","S":"Sell","v":"0.25","p":"16578.00",
             "L":"MinusTick","i":"1c0f43950-d8dd-5b31-9112-a178eb6023ff","BT":false}
        ]
    }"#;

    #[test]
    fn taker_side_maps_through_without_inversion() {
        assert_eq!(aggressor_from_taker_side("Buy").unwrap(), Side::Buy);
        assert_eq!(aggressor_from_taker_side("Sell").unwrap(), Side::Sell);
        assert!(aggressor_from_taker_side("buy").is_err(), "casing is exact");
        assert!(aggressor_from_taker_side("").is_err());
    }

    #[test]
    fn parses_a_multi_trade_message() {
        let events = parse_message(TRADES).unwrap();
        assert_eq!(events.len(), 2, "one message, two trades");

        let MarketEvent::Trade(first) = &events[0] else {
            panic!("expected a trade");
        };
        assert_eq!(first.aggressor, Side::Buy);
        assert_eq!(first.price, Price::parse("16578.50").unwrap());
        assert_eq!(first.qty, Qty::parse("0.001").unwrap());
        assert_eq!(first.ts, Ts::from_millis(1_672_304_486_865));

        let MarketEvent::Trade(second) = &events[1] else {
            panic!("expected a trade");
        };
        assert_eq!(second.aggressor, Side::Sell);
        assert!(second.signed_qty().is_negative());
    }

    #[test]
    fn uses_the_per_trade_timestamp_not_the_envelope() {
        let events = parse_message(TRADES).unwrap();
        let MarketEvent::Trade(t) = &events[0] else {
            panic!()
        };
        // Envelope ts is ...868; the trade's own T is ...865.
        assert_eq!(t.ts, Ts::from_millis(1_672_304_486_865));
    }

    #[test]
    fn parses_an_orderbook_snapshot() {
        let raw = r#"{
            "topic":"orderbook.50.BTCUSDT","type":"snapshot","ts":1672304484978,
            "data":{"s":"BTCUSDT",
                "b":[["16493.50","0.006"],["16493.00","0.100"]],
                "a":[["16611.00","0.029"]],
                "u":18521288,"seq":7961638724}
        }"#;
        let events = parse_message(raw).unwrap();
        let MarketEvent::BookSnapshot {
            ts,
            bids,
            asks,
            sequence,
        } = &events[0]
        else {
            panic!("expected a snapshot, got {:?}", events[0]);
        };
        assert_eq!(*ts, Ts::from_millis(1_672_304_484_978));
        assert_eq!(*sequence, 18_521_288);
        assert_eq!(bids.len(), 2);
        assert_eq!(asks.len(), 1);
    }

    #[test]
    fn snapshot_and_delta_are_distinguished_by_type() {
        let base = r#"{"topic":"orderbook.50.BTCUSDT","type":"TYPE","ts":1,
            "data":{"s":"BTCUSDT","b":[["100","1"]],"a":[],"u":5,"seq":9}}"#;

        let snap = parse_message(&base.replace("TYPE", "snapshot")).unwrap();
        assert!(matches!(snap[0], MarketEvent::BookSnapshot { .. }));

        let delta = parse_message(&base.replace("TYPE", "delta")).unwrap();
        assert!(matches!(delta[0], MarketEvent::BookDelta { .. }));

        // An unrecognised type must not be quietly treated as a delta: doing
        // so on a real snapshot would leave stale levels in the book forever.
        let bad = parse_message(&base.replace("TYPE", "patch"));
        assert!(matches!(bad, Err(FeedError::BadField { field: "type", .. })));
    }

    #[test]
    fn zero_quantity_levels_survive_as_removals() {
        let raw = r#"{"topic":"orderbook.50.BTCUSDT","type":"delta","ts":1,
            "data":{"s":"BTCUSDT","b":[["100","0"]],"a":[],"u":6,"seq":9}}"#;
        let events = parse_message(raw).unwrap();
        let MarketEvent::BookDelta { bids, .. } = &events[0] else {
            panic!()
        };
        assert_eq!(bids[0].1, Qty::ZERO);
    }

    #[test]
    fn non_market_messages_are_reported_as_unhandled() {
        let ack = r#"{"success":true,"ret_msg":"subscribe","op":"subscribe"}"#;
        assert!(matches!(parse_message(ack), Err(FeedError::Unhandled(_))));

        let other = r#"{"topic":"kline.1.BTCUSDT","type":"snapshot","ts":1,"data":[]}"#;
        assert!(matches!(parse_message(other), Err(FeedError::Unhandled(t)) if t.starts_with("kline")));
    }

    #[test]
    fn builds_topic_names() {
        assert_eq!(trade_topic("btcusdt"), "publicTrade.BTCUSDT");
        assert_eq!(orderbook_topic("btcusdt", 50), "orderbook.50.BTCUSDT");
    }
}
