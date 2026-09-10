//! Market data feed adapters.
//!
//! The crate is split so that everything which can be wrong silently is pure
//! and testable. Wire-format translation — where a misread field inverts every
//! delta in the platform — lives in [`binance`] and [`bybit`] as functions over
//! strings, verified against recorded payloads. Transport (sockets, reconnect,
//! backfill) is a thin shell that feeds those functions.
//!
//! [`replay`] and [`synthetic`] provide feeds that need no network at all,
//! which is how the rest of the platform is developed and tested.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod binance;
pub mod bybit;
pub mod error;
pub mod json;
pub mod replay;
pub mod synthetic;

pub use error::FeedError;
pub use replay::ReplayFeed;
pub use synthetic::SyntheticFeed;

use atas_core::MarketEvent;

/// A pull-based source of normalised market events.
///
/// Pull rather than push keeps backpressure with the consumer: if the
/// aggregation loop falls behind, events queue in the transport instead of
/// piling up in an unbounded channel until memory runs out.
pub trait Feed {
    /// Take the next event, or `None` when the source is exhausted.
    ///
    /// A live feed returns `None` only when it has been shut down; a replay
    /// returns `None` at the end of its window.
    fn next_event(&mut self) -> Result<Option<MarketEvent>, FeedError>;
}

/// Drain a feed into a vector. Intended for tests and bounded replays.
pub fn drain(feed: &mut impl Feed) -> Result<Vec<MarketEvent>, FeedError> {
    let mut out = Vec::new();
    while let Some(event) = feed.next_event()? {
        out.push(event);
    }
    Ok(out)
}
