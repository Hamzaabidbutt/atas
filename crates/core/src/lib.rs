//! Domain types shared by every layer of the ATAS order-flow platform.
//!
//! Nothing in this crate performs I/O or allocates background work. It defines
//! the vocabulary — prices, quantities, instruments, trades and books — that
//! the aggregation engine, storage, feeds and trading layers all speak.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod book;
pub mod fixed;
pub mod instrument;
pub mod market;
pub mod time;

pub use book::{BookLevel, BookSide, OrderBook};
pub use fixed::{ParseFixedError, Price, Qty, Rounding, DECIMALS, SCALE};
pub use instrument::{Instrument, InstrumentKind, Venue};
pub use market::{MarketEvent, Side, Trade};
pub use time::Ts;
