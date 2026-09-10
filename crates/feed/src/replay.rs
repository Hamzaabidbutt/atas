//! Replay a stored tick history as a feed.
//!
//! This is how the platform is developed and how a trader rehearses a setup:
//! the same aggregation and indicator code runs, fed from disk instead of a
//! socket. It reads in time windows rather than loading a session into memory,
//! so replaying a busy day costs a bounded working set.

use atas_core::{Instrument, MarketEvent, Ts};
use atas_store::TickStore;

use crate::error::FeedError;
use crate::Feed;

/// Default replay window: one second of history per read.
pub const DEFAULT_CHUNK_NANOS: i64 = atas_core::time::NANOS_PER_SEC;

/// A feed that emits stored trades in time order.
#[derive(Debug)]
pub struct ReplayFeed {
    store: TickStore,
    start: Ts,
    cursor: Ts,
    end: Ts,
    chunk_nanos: i64,
    buffer: std::collections::VecDeque<MarketEvent>,
    emitted: u64,
}

impl ReplayFeed {
    /// Replay `[from, to)` from an open store.
    pub fn new(store: TickStore, from: Ts, to: Ts) -> Self {
        Self {
            store,
            start: from,
            cursor: from,
            end: to,
            chunk_nanos: DEFAULT_CHUNK_NANOS,
            buffer: std::collections::VecDeque::new(),
            emitted: 0,
        }
    }

    /// Replay an instrument's entire stored history.
    pub fn all(store: TickStore) -> Self {
        let from = store.first_ts().unwrap_or(Ts::EPOCH);
        // The store's range is half-open, so reach one nanosecond past the
        // last record to include it.
        let to = store.last_ts().map_or(Ts::EPOCH, |t| t + 1);
        Self::new(store, from, to)
    }

    /// Open an instrument's history beneath `root` and replay all of it.
    pub fn open(
        root: impl AsRef<std::path::Path>,
        instrument: &Instrument,
    ) -> Result<Self, FeedError> {
        Ok(Self::all(TickStore::open(root, instrument)?))
    }

    /// Set how much history is read per window.
    pub fn with_chunk_nanos(mut self, nanos: i64) -> Self {
        assert!(nanos > 0, "replay chunk must be positive");
        self.chunk_nanos = nanos;
        self
    }

    /// How many events have been emitted so far.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Whether the replay has reached its end.
    pub fn is_finished(&self) -> bool {
        self.buffer.is_empty() && self.cursor >= self.end
    }

    /// How far through the replay window the cursor has read, in `0.0..=1.0`.
    ///
    /// This tracks the window, not the buffer: events already read from disk
    /// but not yet taken by the caller count as done.
    pub fn progress(&self) -> f64 {
        let total = self.end - self.start;
        if total <= 0 {
            return 1.0;
        }
        let done = (self.cursor - self.start).clamp(0, total);
        done as f64 / total as f64
    }

    /// The window this feed replays, as `[from, to)`.
    pub fn window(&self) -> (Ts, Ts) {
        (self.start, self.end)
    }

    /// Fill the buffer from the next non-empty window, if any remain.
    ///
    /// An empty window doubles the span for the next attempt. Without that,
    /// skipping a quiet stretch costs one scan per chunk: an overnight gap at
    /// a one-second chunk is 30,000 wasted scans, and a chunk small relative
    /// to the replay span degenerates into an effectively infinite loop.
    /// Doubling bounds the empty scans to O(log span); the window resets to
    /// the configured chunk as soon as data is found, so a dense replay keeps
    /// its bounded working set.
    fn fill(&mut self) -> Result<(), FeedError> {
        let mut span = self.chunk_nanos;

        while self.buffer.is_empty() && self.cursor < self.end {
            let window_end = Ts::from_nanos(
                self.cursor
                    .nanos()
                    .saturating_add(span)
                    .min(self.end.nanos()),
            );

            let buffer = &mut self.buffer;
            self.store.scan(self.cursor, window_end, |trade| {
                buffer.push_back(MarketEvent::Trade(trade));
            })?;
            self.cursor = window_end;

            if self.buffer.is_empty() {
                span = span.saturating_mul(2);
            }
        }
        Ok(())
    }
}

impl Feed for ReplayFeed {
    fn next_event(&mut self) -> Result<Option<MarketEvent>, FeedError> {
        self.fill()?;
        let event = self.buffer.pop_front();
        if event.is_some() {
            self.emitted += 1;
        }
        Ok(event)
    }
}
