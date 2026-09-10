//! Bar aggregation and cluster (footprint) computation.
//!
//! This crate turns a stream of [`atas_core::Trade`]s into the structures a
//! chart draws: bars built by several different rules, each carrying the
//! per-price volume ladder that makes a footprint chart a footprint chart.
//!
//! Nothing here does I/O or spawns work — feed it trades, take back bars.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod aggregator;
pub mod bar;
pub mod cluster;

pub use aggregator::{Aggregator, BarSpec, SpecError};
pub use bar::Bar;
pub use cluster::{
    Cluster, ClusterLadder, ClusterRow, Imbalance, LadderSpec, LadderSpecError, ValueArea,
};
