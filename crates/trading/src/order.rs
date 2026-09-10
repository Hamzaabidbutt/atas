//! Orders and fills.

use atas_core::{Price, Qty, Side, Ts};
use serde::{Deserialize, Serialize};

/// Identifier assigned by the matching engine.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct OrderId(pub u64);

impl std::fmt::Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// What kind of order this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrderType {
    /// Cross the spread now, taking whatever the book offers.
    Market,
    /// Rest until the market trades through `price`.
    Limit {
        /// The limit price.
        price: Price,
    },
    /// Become a market order once the market trades at or through `trigger`.
    Stop {
        /// The trigger price.
        trigger: Price,
    },
    /// Become a limit order once the market trades at or through `trigger`.
    StopLimit {
        /// The trigger price.
        trigger: Price,
        /// The limit price applied after triggering.
        limit: Price,
    },
}

impl OrderType {
    /// The price this order rests at, if it rests.
    pub fn limit_price(&self) -> Option<Price> {
        match self {
            OrderType::Limit { price } => Some(*price),
            OrderType::StopLimit { limit, .. } => Some(*limit),
            _ => None,
        }
    }

    /// The price this order triggers at, if it triggers.
    pub fn trigger_price(&self) -> Option<Price> {
        match self {
            OrderType::Stop { trigger } | OrderType::StopLimit { trigger, .. } => Some(*trigger),
            _ => None,
        }
    }
}

/// How long an order stays live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeInForce {
    /// Rest until filled or cancelled.
    #[default]
    Gtc,
    /// Fill whatever is available immediately, cancel the rest.
    Ioc,
    /// Fill completely and immediately, or cancel entirely.
    Fok,
}

/// Where an order is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Waiting for its trigger price.
    Pending,
    /// Live and eligible to fill.
    Working,
    /// Some quantity filled, the rest still working.
    PartiallyFilled,
    /// Completely filled.
    Filled,
    /// Cancelled before completing.
    Cancelled,
    /// Refused at submission.
    Rejected,
}

impl OrderStatus {
    /// Whether the order can still trade.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            OrderStatus::Pending | OrderStatus::Working | OrderStatus::PartiallyFilled
        )
    }

    /// Whether the order has reached a final state.
    pub fn is_done(self) -> bool {
        !self.is_live()
    }
}

/// A submitted order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    /// Engine-assigned identifier.
    pub id: OrderId,
    /// Buy or sell.
    pub side: Side,
    /// Order type and its prices.
    pub kind: OrderType,
    /// Requested quantity.
    pub qty: Qty,
    /// Quantity filled so far.
    pub filled: Qty,
    /// Volume-weighted average fill price, once anything has filled.
    pub avg_fill_price: Option<Price>,
    /// Current status.
    pub status: OrderStatus,
    /// Time in force.
    pub tif: TimeInForce,
    /// When it was submitted.
    pub created: Ts,
    /// When it reached a final state.
    pub closed: Option<Ts>,
    /// OCO group; filling any member cancels the others.
    pub oco_group: Option<u64>,
    /// Why it was rejected, when it was.
    pub reject_reason: Option<String>,
}

impl Order {
    /// Quantity still working.
    #[inline]
    pub fn remaining(&self) -> Qty {
        self.qty - self.filled
    }

    /// Whether every unit has filled.
    #[inline]
    pub fn is_complete(&self) -> bool {
        self.filled >= self.qty
    }
}

/// A request to place an order.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderRequest {
    /// Buy or sell.
    pub side: Side,
    /// Order type and prices.
    pub kind: OrderType,
    /// Quantity.
    pub qty: Qty,
    /// Time in force.
    pub tif: TimeInForce,
    /// OCO group to join.
    pub oco_group: Option<u64>,
}

impl OrderRequest {
    /// A market order.
    pub fn market(side: Side, qty: Qty) -> Self {
        Self {
            side,
            kind: OrderType::Market,
            qty,
            tif: TimeInForce::Gtc,
            oco_group: None,
        }
    }

    /// A resting limit order.
    pub fn limit(side: Side, qty: Qty, price: Price) -> Self {
        Self {
            side,
            kind: OrderType::Limit { price },
            qty,
            tif: TimeInForce::Gtc,
            oco_group: None,
        }
    }

    /// A stop order that becomes a market order when triggered.
    pub fn stop(side: Side, qty: Qty, trigger: Price) -> Self {
        Self {
            side,
            kind: OrderType::Stop { trigger },
            qty,
            tif: TimeInForce::Gtc,
            oco_group: None,
        }
    }

    /// Set the time in force.
    pub fn with_tif(mut self, tif: TimeInForce) -> Self {
        self.tif = tif;
        self
    }

    /// Join an OCO group.
    pub fn with_oco(mut self, group: u64) -> Self {
        self.oco_group = Some(group);
        self
    }
}

/// An execution against an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    /// Which order filled.
    pub order_id: OrderId,
    /// When.
    pub ts: Ts,
    /// At what price.
    pub price: Price,
    /// How much.
    pub qty: Qty,
    /// Which side the filling order was.
    pub side: Side,
    /// Commission charged, in quote currency.
    pub commission: Qty,
}

/// Why an order was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RejectReason {
    /// Quantity was zero or negative.
    #[error("order quantity must be positive")]
    NonPositiveQty,
    /// A limit or stop price was zero or negative.
    #[error("order price must be positive")]
    NonPositivePrice,
    /// The order would exceed the configured position limit.
    #[error("order would breach the position limit of {limit}")]
    PositionLimit {
        /// The configured limit.
        limit: Qty,
    },
    /// There were already too many working orders.
    #[error("too many working orders (limit {limit})")]
    TooManyOrders {
        /// The configured limit.
        limit: usize,
    },
    /// No book was available to price against.
    #[error("no market data available to price the order")]
    NoMarket,
}
