//! Market events: the normalised stream every feed adapter produces.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fixed::{Price, Qty};
use crate::time::Ts;

/// Which side initiated a trade, or which side of the book a level sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Buyer was the aggressor — the trade lifted the ask.
    Buy,
    /// Seller was the aggressor — the trade hit the bid.
    Sell,
}

impl Side {
    /// The opposite side.
    #[inline]
    pub const fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    /// `+1` for a buy, `-1` for a sell. Multiply a size by this to get delta.
    #[inline]
    pub const fn signum(self) -> i64 {
        match self {
            Side::Buy => 1,
            Side::Sell => -1,
        }
    }

    /// Whether this is the buy side.
    #[inline]
    pub const fn is_buy(self) -> bool {
        matches!(self, Side::Buy)
    }

    /// Stable lower-case identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Side::Buy => "buy",
            Side::Sell => "sell",
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single executed trade.
///
/// `aggressor` is the whole point of the struct: without knowing which side
/// crossed the spread there is no delta, no imbalance, and no footprint — just
/// volume. Feeds that report it as "buyer is maker" must invert it on the way
/// in, and adapters are responsible for that normalisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
    /// Exchange timestamp.
    pub ts: Ts,
    /// Execution price.
    pub price: Price,
    /// Executed size.
    pub qty: Qty,
    /// The side that crossed the spread.
    pub aggressor: Side,
    /// Venue trade id, used to deduplicate across REST backfill and websocket
    /// overlap. Zero when the venue does not supply one.
    pub id: u64,
}

impl Trade {
    /// Construct a trade.
    pub const fn new(ts: Ts, price: Price, qty: Qty, aggressor: Side, id: u64) -> Self {
        Self {
            ts,
            price,
            qty,
            aggressor,
            id,
        }
    }

    /// Signed size: positive when buyers were aggressive, negative when sellers
    /// were. Summing this across trades gives delta.
    #[inline]
    pub fn signed_qty(&self) -> Qty {
        Qty::from_minor(self.qty.minor() * self.aggressor.signum())
    }

    /// Notional traded value.
    #[inline]
    pub fn notional(&self) -> Qty {
        self.price.notional(self.qty)
    }
}

/// A normalised event from any feed.
///
/// Adapters translate venue-specific wire formats into this enum, so the
/// engine, storage and trading layers never learn what a venue's JSON looks
/// like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MarketEvent {
    /// A completed trade.
    Trade(Trade),
    /// A full order book replacement. Sent on connect and after any gap.
    BookSnapshot {
        /// Exchange timestamp.
        ts: Ts,
        /// Bid levels, any order.
        bids: Vec<(Price, Qty)>,
        /// Ask levels, any order.
        asks: Vec<(Price, Qty)>,
        /// Venue sequence number, if it supplies one.
        sequence: u64,
    },
    /// An incremental book update. A quantity of zero removes the level.
    BookDelta {
        /// Exchange timestamp.
        ts: Ts,
        /// Changed bid levels.
        bids: Vec<(Price, Qty)>,
        /// Changed ask levels.
        asks: Vec<(Price, Qty)>,
        /// Venue sequence number, if it supplies one.
        sequence: u64,
    },
    /// Connection state changed. The engine uses this to mark gaps in history
    /// rather than silently stitching across a disconnect.
    Status {
        /// When the change was observed.
        ts: Ts,
        /// Whether the feed is currently connected.
        connected: bool,
        /// Human-readable detail for the UI.
        detail: String,
    },
}

impl MarketEvent {
    /// Timestamp of any event variant.
    pub fn ts(&self) -> Ts {
        match self {
            MarketEvent::Trade(t) => t.ts,
            MarketEvent::BookSnapshot { ts, .. }
            | MarketEvent::BookDelta { ts, .. }
            | MarketEvent::Status { ts, .. } => *ts,
        }
    }

    /// Whether this event is a trade.
    pub fn is_trade(&self) -> bool {
        matches!(self, MarketEvent::Trade(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(px: &str, qty: &str, side: Side) -> Trade {
        Trade::new(
            Ts::from_millis(1_700_000_000_000),
            Price::parse(px).unwrap(),
            Qty::parse(qty).unwrap(),
            side,
            1,
        )
    }

    #[test]
    fn side_inverts_and_signs() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
        assert_eq!(Side::Buy.signum(), 1);
        assert_eq!(Side::Sell.signum(), -1);
        assert!(Side::Buy.is_buy());
    }

    #[test]
    fn signed_qty_encodes_aggression() {
        let buy = trade("100", "2", Side::Buy);
        let sell = trade("100", "3", Side::Sell);
        assert_eq!(buy.signed_qty(), Qty::parse("2").unwrap());
        assert_eq!(sell.signed_qty(), Qty::parse("-3").unwrap());

        // Delta is just the sum of signed sizes.
        let delta = buy.signed_qty() + sell.signed_qty();
        assert_eq!(delta, Qty::parse("-1").unwrap());
    }

    #[test]
    fn computes_notional() {
        assert_eq!(
            trade("95000", "0.5", Side::Buy).notional(),
            Qty::parse("47500").unwrap()
        );
    }

    #[test]
    fn events_expose_their_timestamp() {
        let t = trade("100", "1", Side::Buy);
        assert_eq!(MarketEvent::Trade(t).ts(), t.ts);

        let status = MarketEvent::Status {
            ts: Ts::from_secs(5),
            connected: false,
            detail: "socket closed".into(),
        };
        assert_eq!(status.ts(), Ts::from_secs(5));
        assert!(!status.is_trade());
    }

    #[test]
    fn events_round_trip_through_json() {
        let t = trade("95000.25", "1.5", Side::Sell);
        let event = MarketEvent::Trade(t);
        let json = serde_json::to_string(&event).unwrap();
        let back: MarketEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }
}
