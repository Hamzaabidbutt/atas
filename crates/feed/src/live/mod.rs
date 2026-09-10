//! Live websocket and REST transport.
//!
//! # What this layer is and is not
//!
//! Everything that can be wrong silently already lives elsewhere in this
//! crate, under test: wire parsing in [`crate::binance`] and [`crate::bybit`],
//! the depth handshake in [`crate::sync`], and reconnect pacing in
//! [`crate::backoff`]. This module only moves bytes between a socket and those
//! functions, so that the part which cannot be tested without a network is as
//! small and as boring as possible.
//!
//! **This module has not been run against a live venue.** It was developed in
//! an environment with no exchange network access, so it is compile-verified
//! and built on tested components, but the first real socket it opens will be
//! on someone else's machine. Treat first-run behaviour accordingly.

use std::sync::mpsc::{Receiver, TryRecvError};

use atas_core::MarketEvent;

use crate::error::FeedError;
use crate::Feed;

pub mod binance;
pub mod bybit;

/// Binance's market-data-only websocket host.
///
/// Deliberately not `stream.binance.com`. The trading hosts are geo-restricted
/// and answer 451 from several jurisdictions including the United States,
/// which is where most cloud and CI networks live. The `.vision` hosts serve
/// the same public market data with no trading capability attached, so they
/// are both the correct endpoint for a client that never places an order here
/// and the one that is actually reachable.
pub const BINANCE_WS_DATA: &str = "wss://data-stream.binance.vision/stream?streams=";

/// Binance's market-data-only REST host. See [`BINANCE_WS_DATA`].
pub const BINANCE_REST_DATA: &str = "https://data-api.binance.vision";

/// Binance's trading websocket host. Geo-restricted; use only where the
/// client genuinely needs the trading endpoints.
pub const BINANCE_WS_TRADE: &str = "wss://stream.binance.com:9443/stream?streams=";

/// Binance's trading REST host. Geo-restricted; see [`BINANCE_WS_TRADE`].
pub const BINANCE_REST_TRADE: &str = "https://api.binance.com";

/// Connection tuning shared by the venue drivers.
#[derive(Debug, Clone)]
pub struct LiveOptions {
    /// Websocket base URL, ending in whatever prefix the venue expects before
    /// the stream list.
    pub ws_base: String,
    /// REST base URL, with no trailing slash.
    pub rest_base: String,
    /// Depth levels to request in the REST snapshot.
    pub snapshot_depth: u32,
    /// How long to wait for a websocket message before treating the
    /// connection as dead.
    ///
    /// A TCP connection can stay open long after it stops delivering data, so
    /// waiting for the socket to report an error can mean waiting for ever.
    pub read_timeout: std::time::Duration,
    /// Bound on the event channel. Beyond this, the producer blocks rather
    /// than letting a slow consumer grow the queue without limit.
    pub channel_capacity: usize,
    /// Backoff used between reconnect attempts.
    pub backoff: crate::backoff::Backoff,
}

impl Default for LiveOptions {
    fn default() -> Self {
        Self {
            ws_base: BINANCE_WS_DATA.to_string(),
            rest_base: BINANCE_REST_DATA.to_string(),
            snapshot_depth: 1_000,
            read_timeout: std::time::Duration::from_secs(30),
            channel_capacity: 65_536,
            backoff: crate::backoff::Backoff::default(),
        }
    }
}

/// A [`Feed`] draining events produced by a background connection task.
///
/// Pull-based on purpose: if the aggregation loop falls behind, events queue
/// in this channel and the producer eventually blocks, rather than the process
/// growing an unbounded backlog until it is killed.
#[derive(Debug)]
pub struct ChannelFeed {
    receiver: Receiver<MarketEvent>,
    /// Kept so dropping the feed signals the producer task to stop.
    _shutdown: ShutdownGuard,
    disconnected: bool,
}

impl ChannelFeed {
    /// Wrap a receiver and its shutdown handle.
    pub fn new(receiver: Receiver<MarketEvent>, shutdown: ShutdownGuard) -> Self {
        Self {
            receiver,
            _shutdown: shutdown,
            disconnected: false,
        }
    }

    /// Whether the producer has stopped and the queue is drained.
    pub fn is_finished(&self) -> bool {
        self.disconnected
    }
}

impl Feed for ChannelFeed {
    fn next_event(&mut self) -> Result<Option<MarketEvent>, FeedError> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.disconnected = true;
                Ok(None)
            }
        }
    }
}

/// Dropping this tells the connection task to shut down.
#[derive(Debug)]
pub struct ShutdownGuard {
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ShutdownGuard {
    /// A guard and the flag its task should poll.
    pub fn new() -> (Self, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        (Self { flag: flag.clone() }, flag)
    }

    /// Signal shutdown without dropping the guard.
    pub fn stop(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Errors specific to running a live connection.
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    /// The websocket failed.
    #[error("websocket error: {0}")]
    WebSocket(String),
    /// An HTTP request failed.
    #[error("http error: {0}")]
    Http(String),
    /// No message arrived within the read timeout.
    #[error("no data for {0:?}; treating the connection as dead")]
    ReadTimeout(std::time::Duration),
    /// Translating a venue message failed.
    #[error(transparent)]
    Feed(#[from] FeedError),
}
