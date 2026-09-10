//! Serialisable views of session state, shaped for the UI.
//!
//! # Why prices cross the boundary as integers
//!
//! Every price and quantity is sent as its fixed-point minor-unit count, not
//! as a decimal. JavaScript numbers are IEEE doubles, which represent integers
//! exactly up to 2^53 (about 9.0e15); minor units at 8 decimal places put a
//! six-figure price at roughly 9.5e12, four orders of magnitude inside that
//! bound. So the integer arrives exact, and the UI divides by [`SCALE`] only
//! at the moment it draws or formats. Sending `95000.01` as a JSON decimal
//! would hand the UI a float to compare against, reintroducing at the boundary
//! exactly the problem fixed point exists to avoid.
//!
//! These types are `Serialize` only. They travel one way — Rust builds them,
//! the UI renders them — and several carry `&'static str` tags that cannot be
//! deserialised without allocating. Commands coming back the other way have
//! their own request types rather than round-tripping a view model, so the UI
//! can never hand back a mutated snapshot and have it treated as truth.

use atas_core::{Price, Qty, Side, Trade, Ts, SCALE};
use atas_engine::{Bar, ClusterRow, Imbalance};
use atas_trading::{Fill, Order, Position};
use serde::Serialize;

/// Minor units per whole unit, so the UI can convert without hard-coding it.
pub const PRICE_SCALE: i64 = SCALE;

/// One price level of a footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClusterDto {
    /// Price, in minor units.
    pub price: i64,
    /// Volume traded on the bid, in minor units.
    pub bid: i64,
    /// Volume traded on the ask, in minor units.
    pub ask: i64,
    /// Trades at this level.
    pub trades: u32,
}

impl From<ClusterRow> for ClusterDto {
    fn from(row: ClusterRow) -> Self {
        Self {
            price: row.price.minor(),
            bid: row.cluster.bid.minor(),
            ask: row.cluster.ask.minor(),
            trades: row.cluster.trades,
        }
    }
}

/// An imbalance marker for the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ImbalanceDto {
    /// Price, in minor units.
    pub price: i64,
    /// `"buy"` or `"sell"`.
    pub side: &'static str,
    /// Dominant over opposing volume; `null` when the opposing side was empty.
    pub ratio: Option<f64>,
}

impl From<&Imbalance> for ImbalanceDto {
    fn from(i: &Imbalance) -> Self {
        Self {
            price: i.price.minor(),
            side: i.side.as_str(),
            ratio: i.ratio.is_finite().then_some(i.ratio),
        }
    }
}

/// A bar with everything the chart needs to draw it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarDto {
    /// Bar open time, nanoseconds since the epoch.
    pub open_ts: i64,
    /// Last trade time in the bar.
    pub close_ts: i64,
    /// Open price, minor units.
    pub open: i64,
    /// High price, minor units.
    pub high: i64,
    /// Low price, minor units.
    pub low: i64,
    /// Close price, minor units.
    pub close: i64,
    /// Total volume, minor units.
    pub volume: i64,
    /// Ask minus bid volume, minor units.
    pub delta: i64,
    /// Trade count.
    pub trades: u32,
    /// Whether the bar is finished.
    pub closed: bool,
    /// Point of control, minor units, if any volume traded.
    pub poc: Option<i64>,
    /// The footprint ladder, low price to high.
    pub clusters: Vec<ClusterDto>,
    /// Diagonal imbalances detected in this bar.
    pub imbalances: Vec<ImbalanceDto>,
}

impl BarDto {
    /// Build from a bar, detecting imbalances at the given ratio.
    pub fn from_bar(bar: &Bar, imbalance_ratio: f64, min_imbalance_volume: Qty) -> Self {
        Self {
            open_ts: bar.open_ts.nanos(),
            close_ts: bar.close_ts.nanos(),
            open: bar.open.minor(),
            high: bar.high.minor(),
            low: bar.low.minor(),
            close: bar.close.minor(),
            volume: bar.volume.minor(),
            delta: bar.delta().minor(),
            trades: bar.trades,
            closed: bar.closed,
            poc: bar.poc().map(|p| p.minor()),
            clusters: bar.clusters.rows().map(ClusterDto::from).collect(),
            imbalances: bar
                .clusters
                .imbalances(imbalance_ratio, min_imbalance_volume)
                .iter()
                .map(ImbalanceDto::from)
                .collect(),
        }
    }
}

/// A depth-of-market level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LevelDto {
    /// Price, minor units.
    pub price: i64,
    /// Resting size, minor units.
    pub qty: i64,
}

/// A snapshot of the order book.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BookDto {
    /// Bid levels, best first.
    pub bids: Vec<LevelDto>,
    /// Ask levels, best first.
    pub asks: Vec<LevelDto>,
    /// Best bid minus best ask, minor units, when both sides exist.
    pub spread: Option<i64>,
    /// Whether the book is crossed, meaning the feed is inconsistent.
    pub crossed: bool,
}

/// A time-and-sales row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TapeRowDto {
    /// Trade time, nanoseconds.
    pub ts: i64,
    /// Price, minor units.
    pub price: i64,
    /// Size, minor units.
    pub qty: i64,
    /// `"buy"` or `"sell"`, the aggressor.
    pub side: &'static str,
}

impl From<&Trade> for TapeRowDto {
    fn from(t: &Trade) -> Self {
        Self {
            ts: t.ts.nanos(),
            price: t.price.minor(),
            qty: t.qty.minor(),
            side: t.aggressor.as_str(),
        }
    }
}

/// Indicator readings at the current moment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct IndicatorsDto {
    /// Cumulative volume delta, minor units.
    pub cvd: i64,
    /// `"bearish"`, `"bullish"` or `null`.
    pub divergence: Option<&'static str>,
    /// Session VWAP, minor units.
    pub vwap: Option<i64>,
    /// One standard deviation around VWAP, minor units.
    pub vwap_deviation: Option<i64>,
    /// Session point of control, minor units.
    pub session_poc: Option<i64>,
    /// Value area high, minor units.
    pub value_area_high: Option<i64>,
    /// Value area low, minor units.
    pub value_area_low: Option<i64>,
    /// Trades per second over the rolling window.
    pub speed_of_tape: f64,
}

/// Position and PnL for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PositionDto {
    /// Signed size, minor units. Positive is long.
    pub qty: i64,
    /// Average entry, minor units.
    pub avg_price: i64,
    /// Realised PnL, minor units.
    pub realised: i64,
    /// Unrealised PnL at the last trade, minor units.
    pub unrealised: i64,
    /// Commission paid, minor units.
    pub commission: i64,
    /// `"buy"`, `"sell"` or `null` when flat.
    pub side: Option<&'static str>,
}

impl PositionDto {
    /// Build from a position, marked at `mark`.
    pub fn from_position(position: &Position, mark: Option<Price>) -> Self {
        Self {
            qty: position.qty.minor(),
            avg_price: position.avg_price.minor(),
            realised: position.realised.minor(),
            unrealised: mark
                .map(|p| position.unrealised(p).minor())
                .unwrap_or(0),
            commission: position.commission.minor(),
            side: position.side().map(Side::as_str),
        }
    }
}

/// A working or completed order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrderDto {
    /// Engine-assigned id.
    pub id: u64,
    /// `"buy"` or `"sell"`.
    pub side: &'static str,
    /// `"market"`, `"limit"`, `"stop"` or `"stop_limit"`.
    pub kind: &'static str,
    /// Requested size, minor units.
    pub qty: i64,
    /// Filled size, minor units.
    pub filled: i64,
    /// Resting price, minor units, when the order has one.
    pub limit_price: Option<i64>,
    /// Trigger price, minor units, when the order has one.
    pub trigger_price: Option<i64>,
    /// Lower-case status name.
    pub status: &'static str,
}

impl From<&Order> for OrderDto {
    fn from(o: &Order) -> Self {
        use atas_trading::{OrderStatus, OrderType};
        Self {
            id: o.id.0,
            side: o.side.as_str(),
            kind: match o.kind {
                OrderType::Market => "market",
                OrderType::Limit { .. } => "limit",
                OrderType::Stop { .. } => "stop",
                OrderType::StopLimit { .. } => "stop_limit",
            },
            qty: o.qty.minor(),
            filled: o.filled.minor(),
            limit_price: o.kind.limit_price().map(|p| p.minor()),
            trigger_price: o.kind.trigger_price().map(|p| p.minor()),
            status: match o.status {
                OrderStatus::Pending => "pending",
                OrderStatus::Working => "working",
                OrderStatus::PartiallyFilled => "partially_filled",
                OrderStatus::Filled => "filled",
                OrderStatus::Cancelled => "cancelled",
                OrderStatus::Rejected => "rejected",
            },
        }
    }
}

/// A fill, for the executions blotter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FillDto {
    /// Which order filled.
    pub order_id: u64,
    /// When, nanoseconds.
    pub ts: i64,
    /// At what price, minor units.
    pub price: i64,
    /// How much, minor units.
    pub qty: i64,
    /// `"buy"` or `"sell"`.
    pub side: &'static str,
}

impl From<&Fill> for FillDto {
    fn from(f: &Fill) -> Self {
        Self {
            order_id: f.order_id.0,
            ts: f.ts.nanos(),
            price: f.price.minor(),
            qty: f.qty.minor(),
            side: f.side.as_str(),
        }
    }
}

/// Everything the UI needs to render a full frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotDto {
    /// Venue-qualified instrument key.
    pub instrument: String,
    /// Minor units per whole unit.
    pub scale: i64,
    /// The instrument's minimum price increment, minor units.
    pub tick_size: i64,
    /// Height of one footprint row, minor units. May span several ticks.
    pub row_size: i64,
    /// Instrument ticks per footprint row.
    pub ticks_per_row: u32,
    /// Decimal places to show for prices.
    pub price_decimals: u32,
    /// Completed bars, oldest first, then the forming bar last.
    pub bars: Vec<BarDto>,
    /// Current book.
    pub book: BookDto,
    /// Recent tape, newest last.
    pub tape: Vec<TapeRowDto>,
    /// Indicator readings.
    pub indicators: IndicatorsDto,
    /// Current position.
    pub position: PositionDto,
    /// Live orders.
    pub orders: Vec<OrderDto>,
    /// Last traded price, minor units.
    pub last_price: Option<i64>,
    /// Timestamp of the most recent event, nanoseconds.
    pub last_ts: i64,
    /// Whether the feed currently reports itself connected.
    ///
    /// In the snapshot rather than only in events because the connection can
    /// fail before the UI has subscribed — which is precisely when a feed
    /// that cannot reach its venue does fail — and those events are then lost.
    pub connected: bool,
    /// The most recent connection message, empty if there has been none.
    pub connection_detail: String,
}

/// Convert a timestamp for the UI.
pub fn ts_nanos(ts: Ts) -> i64 {
    ts.nanos()
}
