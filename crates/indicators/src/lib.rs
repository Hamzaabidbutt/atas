//! Indicators for the ATAS platform.
//!
//! Order-flow studies live here alongside the classical ones, but they are not
//! the same kind of object: a moving average summarises price, while CVD,
//! profiles and the scanners summarise *who was aggressive and where*. The
//! latter is the reason the platform exists, so those get the detail.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod classic;
pub mod cvd;
pub mod profile;
pub mod scanners;
pub mod traits;
pub mod vwap;

pub use classic::{Atr, Ema, Rsi, Sma};
pub use cvd::{Cvd, Divergence};
pub use profile::{SessionProfile, TpoProfile, DEFAULT_VALUE_AREA};
pub use scanners::{BigTrade, BigTrades, ClusterCriteria, ClusterHit, ClusterSearch, SpeedOfTape};
pub use traits::{BarIndicator, TradeIndicator};
pub use vwap::{Vwap, VwapValue};
