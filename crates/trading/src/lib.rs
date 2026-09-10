//! Paper trading: simulated matching, positions and PnL.
//!
//! The engine models execution against real market data without sending
//! anything anywhere. Its value depends entirely on being pessimistic where a
//! real venue would be — see [`engine`] for the two fill decisions that
//! matter most.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod engine;
pub mod order;
pub mod position;

pub use engine::{PaperEngine, SubmitOutcome, TradingConfig};
pub use order::{
    Fill, Order, OrderId, OrderRequest, OrderStatus, OrderType, RejectReason, TimeInForce,
};
pub use position::Position;
