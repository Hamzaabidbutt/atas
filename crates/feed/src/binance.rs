//! Binance wire format.
//!
//! # The aggressor inversion
//!
//! Binance trade messages carry `m`: *"was the buyer the market maker?"*. When
//! `m` is true the buyer was resting, so the **seller** crossed the spread and
//! the aggressor is [`Side::Sell`]. Reading `m` as "is buy" inverts delta,
//! every imbalance, and the sign of CVD across the entire platform, while
//! still producing plausible-looking charts. It is the single easiest way to
//! get an order-flow integration silently, catastrophically wrong, so it is
//! isolated in [`aggressor_from_maker_flag`] and tested directly.

use atas_core::{MarketEvent, Side, Trade, Ts};
use serde_json::Value;

use crate::error::FeedError;
use crate::json::{int_field, level_array, str_field, uint_field};

/// Translate Binance's `m` flag into the aggressor side.
///
/// `is_buyer_maker == true` means the buyer was resting and the seller lifted
/// into them, so the aggressor is the seller.
#[inline]
pub fn aggressor_from_maker_flag(is_buyer_maker: bool) -> Side {
    if is_buyer_maker {
        Side::Sell
    } else {
        Side::Buy
    }
}

/// Unwrap the `{"stream":..,"data":..}` envelope of a combined stream.
///
/// Single-stream connections send the payload bare, so this is a no-op there.
pub fn unwrap_stream(value: &Value) -> &Value {
    match value.get("data") {
        Some(data) if value.get("stream").is_some() => data,
        _ => value,
    }
}

/// Parse any Binance market message into a normalised event.
pub fn parse_message(raw: &str) -> Result<MarketEvent, FeedError> {
    let value: Value = serde_json::from_str(raw)?;
    parse_value(&value)
}

/// Parse an already-decoded Binance message.
pub fn parse_value(value: &Value) -> Result<MarketEvent, FeedError> {
    let payload = unwrap_stream(value);
    let kind = payload.get("e").and_then(Value::as_str).unwrap_or_default();

    match kind {
        "aggTrade" | "trade" => parse_trade(payload).map(MarketEvent::Trade),
        "depthUpdate" => parse_depth_update(payload),
        other => Err(FeedError::Unhandled(other.to_string())),
    }
}

/// Parse an `aggTrade` or `trade` message.
pub fn parse_trade(payload: &Value) -> Result<Trade, FeedError> {
    const CTX: &str = "binance trade";

    let is_buyer_maker = payload
        .get("m")
        .and_then(Value::as_bool)
        .ok_or(FeedError::MissingField {
            field: "m",
            context: CTX,
        })?;

    // aggTrade carries the aggregate id in `a`; a raw trade carries `t`.
    let id = uint_field(payload, "a", CTX)
        .or_else(|_| uint_field(payload, "t", CTX))
        .unwrap_or(0);

    Ok(Trade {
        // `T` is the trade time; `E` is when the server emitted the event.
        // Bar boundaries must follow the trade, not the transmission.
        ts: Ts::from_millis(int_field(payload, "T", CTX)?),
        price: crate::json::price_field(payload, "p", CTX)?,
        qty: crate::json::qty_field(payload, "q", CTX)?,
        aggressor: aggressor_from_maker_flag(is_buyer_maker),
        id,
    })
}

/// Parse a `depthUpdate` message into a book delta.
///
/// Binance labels each update with the range `[U, u]` of sequence ids it
/// covers. The final id is what the book stores, and the adapter's gap check
/// uses `U` against the previously applied `u`.
pub fn parse_depth_update(payload: &Value) -> Result<MarketEvent, FeedError> {
    const CTX: &str = "binance depthUpdate";
    Ok(MarketEvent::BookDelta {
        ts: Ts::from_millis(int_field(payload, "E", CTX)?),
        bids: level_array(payload, "b", CTX)?,
        asks: level_array(payload, "a", CTX)?,
        sequence: uint_field(payload, "u", CTX)?,
    })
}

/// First sequence id covered by a `depthUpdate`.
///
/// A delta may be applied only if this is at most one past the book's current
/// sequence; a larger value is a gap and requires a fresh REST snapshot.
pub fn depth_update_first_id(payload: &Value) -> Result<u64, FeedError> {
    uint_field(unwrap_stream(payload), "U", "binance depthUpdate")
}

/// Parse a REST `/api/v3/depth` snapshot.
pub fn parse_depth_snapshot(raw: &str, ts: Ts) -> Result<MarketEvent, FeedError> {
    const CTX: &str = "binance depth snapshot";
    let value: Value = serde_json::from_str(raw)?;
    Ok(MarketEvent::BookSnapshot {
        ts,
        bids: level_array(&value, "bids", CTX)?,
        asks: level_array(&value, "asks", CTX)?,
        sequence: uint_field(&value, "lastUpdateId", CTX)?,
    })
}

/// Parse a REST `/api/v3/aggTrades` backfill page.
pub fn parse_agg_trades(raw: &str) -> Result<Vec<Trade>, FeedError> {
    let value: Value = serde_json::from_str(raw)?;
    let arr = value.as_array().ok_or_else(|| FeedError::BadField {
        field: "body",
        context: "binance aggTrades",
        value: value.to_string(),
    })?;
    arr.iter().map(parse_trade).collect()
}

/// Stream name for an instrument's aggregate trades.
pub fn agg_trade_stream(symbol: &str) -> String {
    format!("{}@aggTrade", symbol.to_lowercase())
}

/// Stream name for an instrument's incremental depth at 100ms.
pub fn depth_stream(symbol: &str) -> String {
    format!("{}@depth@100ms", symbol.to_lowercase())
}

/// Symbol carried by a message, when it has one.
pub fn symbol_of(value: &Value) -> Option<&str> {
    str_field(unwrap_stream(value), "s", "binance message").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use atas_core::{Price, Qty};

    /// A real-shaped aggTrade payload.
    const AGG_TRADE: &str = r#"{
        "e":"aggTrade","E":1672515782136,"s":"BTCUSDT","a":26129,
        "p":"16578.50","q":"0.00385","f":27781,"l":27781,
        "T":1672515782130,"m":true,"M":true
    }"#;

    #[test]
    fn maker_flag_inverts_into_the_aggressor() {
        // The whole platform's delta sign hangs on these two lines.
        assert_eq!(aggressor_from_maker_flag(true), Side::Sell);
        assert_eq!(aggressor_from_maker_flag(false), Side::Buy);
    }

    #[test]
    fn parses_an_agg_trade() {
        let value: Value = serde_json::from_str(AGG_TRADE).unwrap();
        let trade = parse_trade(&value).unwrap();

        assert_eq!(trade.price, Price::parse("16578.50").unwrap());
        assert_eq!(trade.qty, Qty::parse("0.00385").unwrap());
        assert_eq!(trade.id, 26129);
        // m == true, so the seller was the aggressor.
        assert_eq!(trade.aggressor, Side::Sell);
        // T (trade time), not E (event time).
        assert_eq!(trade.ts, Ts::from_millis(1_672_515_782_130));
    }

    #[test]
    fn buyer_aggressor_is_recognised() {
        let raw = AGG_TRADE.replace("\"m\":true", "\"m\":false");
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parse_trade(&value).unwrap().aggressor, Side::Buy);
    }

    #[test]
    fn signed_quantity_follows_the_maker_flag() {
        // An end-to-end check that delta comes out with the right sign.
        let value: Value = serde_json::from_str(AGG_TRADE).unwrap();
        let sell = parse_trade(&value).unwrap();
        assert!(sell.signed_qty().is_negative(), "m=true must give -qty");

        let raw = AGG_TRADE.replace("\"m\":true", "\"m\":false");
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert!(parse_trade(&value).unwrap().signed_qty().is_positive());
    }

    #[test]
    fn unwraps_a_combined_stream_envelope() {
        let raw = format!(r#"{{"stream":"btcusdt@aggTrade","data":{AGG_TRADE}}}"#);
        let event = parse_message(&raw).unwrap();
        let MarketEvent::Trade(trade) = event else {
            panic!("expected a trade, got {event:?}");
        };
        assert_eq!(trade.id, 26129);
        assert_eq!(symbol_of(&serde_json::from_str(&raw).unwrap()), Some("BTCUSDT"));
    }

    #[test]
    fn parses_a_depth_update() {
        let raw = r#"{
            "e":"depthUpdate","E":1672515782136,"s":"BTCUSDT",
            "U":157,"u":160,
            "b":[["16578.00","2.5"],["16577.50","0"]],
            "a":[["16579.00","1.25"]]
        }"#;
        let event = parse_message(raw).unwrap();
        let MarketEvent::BookDelta {
            ts,
            bids,
            asks,
            sequence,
        } = event
        else {
            panic!("expected a book delta");
        };

        assert_eq!(ts, Ts::from_millis(1_672_515_782_136));
        assert_eq!(sequence, 160, "the book stores the final id");
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0], (Price::parse("16578.00").unwrap(), Qty::parse("2.5").unwrap()));
        // A zero quantity is a removal and must survive parsing as zero.
        assert_eq!(bids[1].1, Qty::ZERO);
        assert_eq!(asks.len(), 1);

        let value: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(depth_update_first_id(&value).unwrap(), 157);
    }

    #[test]
    fn a_one_sided_depth_update_parses() {
        let raw = r#"{"e":"depthUpdate","E":1,"s":"BTCUSDT","U":1,"u":2,"b":[["100","1"]]}"#;
        let MarketEvent::BookDelta { bids, asks, .. } = parse_message(raw).unwrap() else {
            panic!("expected a book delta");
        };
        assert_eq!(bids.len(), 1);
        assert!(asks.is_empty());
    }

    #[test]
    fn parses_a_rest_depth_snapshot() {
        let raw = r#"{
            "lastUpdateId":1027024,
            "bids":[["16578.00","2.5"]],
            "asks":[["16579.00","1.25"],["16580.00","3"]]
        }"#;
        let event = parse_depth_snapshot(raw, Ts::from_millis(5)).unwrap();
        let MarketEvent::BookSnapshot {
            ts,
            bids,
            asks,
            sequence,
        } = event
        else {
            panic!("expected a snapshot");
        };
        assert_eq!(ts, Ts::from_millis(5));
        assert_eq!(sequence, 1_027_024);
        assert_eq!(bids.len(), 1);
        assert_eq!(asks.len(), 2);
    }

    #[test]
    fn parses_a_rest_backfill_page() {
        let raw = r#"[
            {"a":1,"p":"100.00","q":"1","f":1,"l":1,"T":1000,"m":false,"M":true},
            {"a":2,"p":"100.50","q":"2","f":2,"l":2,"T":2000,"m":true,"M":true}
        ]"#;
        let trades = parse_agg_trades(raw).unwrap();
        assert_eq!(trades.len(), 2);
        assert_eq!(trades[0].aggressor, Side::Buy);
        assert_eq!(trades[1].aggressor, Side::Sell);
        assert!(trades[0].ts <= trades[1].ts, "backfill must stay ordered");
    }

    #[test]
    fn unknown_message_kinds_are_reported_not_guessed() {
        let raw = r#"{"e":"kline","E":1,"s":"BTCUSDT"}"#;
        assert!(matches!(parse_message(raw), Err(FeedError::Unhandled(k)) if k == "kline"));
    }

    #[test]
    fn a_trade_without_the_maker_flag_is_rejected() {
        // Better to fail loudly than to default the aggressor and invent flow.
        let raw = r#"{"e":"aggTrade","E":1,"s":"BTCUSDT","a":1,"p":"1","q":"1","T":1}"#;
        assert!(matches!(
            parse_message(raw),
            Err(FeedError::MissingField { field: "m", .. })
        ));
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(matches!(parse_message("{not json"), Err(FeedError::Json(_))));
    }

    #[test]
    fn builds_stream_names() {
        assert_eq!(agg_trade_stream("BTCUSDT"), "btcusdt@aggTrade");
        assert_eq!(depth_stream("BTCUSDT"), "btcusdt@depth@100ms");
    }
}
