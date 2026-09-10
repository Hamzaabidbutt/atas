//! The application layer: one session per instrument, wired end to end.
//!
//! This crate holds everything the desktop app does, in plain Rust with no
//! window toolkit anywhere near it. The Tauri shell is a transport: it hands
//! market events to a [`Session`] and forwards the [`AppEvent`]s that come
//! back. Logic that can only be exercised by launching a window is logic that
//! does not get tested, so none of it lives there.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dto;
pub mod session;

pub use dto::{
    BarDto, BookDto, ClusterDto, FillDto, ImbalanceDto, IndicatorsDto, LevelDto, OrderDto,
    PositionDto, SnapshotDto, TapeRowDto, PRICE_SCALE,
};
pub use session::{AppEvent, Session, SessionConfig};
