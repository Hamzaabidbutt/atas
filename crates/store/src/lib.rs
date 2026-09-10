//! Segmented, append-only tick storage.
//!
//! Cluster charts are built from ticks, not candles, so the platform has to
//! keep raw trade history and re-read it fast. This crate stores trades as
//! fixed-width records in time-ordered segment files, which makes a range
//! query a binary search plus a sequential read — with no index file to keep
//! in sync or rebuild after a crash.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod record;
pub mod segment;
pub mod store;

pub use error::StoreError;
pub use record::{RecordError, RECORD_SIZE};
pub use segment::Segment;
pub use store::{TickStore, DEFAULT_SEGMENT_RECORDS, FORMAT_VERSION};
