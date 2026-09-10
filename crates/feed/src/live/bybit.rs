//! Bybit v5 live connection.
//!
//! Simpler than Binance in one respect and harder in another. Bybit sends its
//! own snapshot over the same socket — no REST handshake to co-ordinate — but
//! it can send a fresh snapshot at any time, and a client that treats one as a
//! delta keeps stale levels in the book for ever. The `type` field is
//! therefore load-bearing, and [`crate::bybit::parse_orderbook`] refuses to
//! guess when it is unrecognised.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use atas_core::{Instrument, MarketEvent, Ts};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::bybit::{self as wire, orderbook_topic, trade_topic};
use crate::live::{ChannelFeed, LiveError, LiveOptions, ShutdownGuard};

/// Bybit's public spot stream. Not geo-restricted the way Binance's trading
/// hosts are, so this stays a constant rather than an option.
const WS_SPOT: &str = "wss://stream.bybit.com/v5/public/spot";

/// Order book depth to subscribe to. Bybit publishes fixed tiers, so this is
/// one of their supported values rather than an arbitrary number.
const BOOK_DEPTH: u32 = 50;

/// Connect to Bybit for one instrument.
///
/// Spawns a background task on the supplied runtime handle and returns a feed
/// draining its output. Dropping the feed stops the task.
pub fn connect(
    instrument: &Instrument,
    options: LiveOptions,
    runtime: &tokio::runtime::Handle,
) -> ChannelFeed {
    let (sender, receiver) = std::sync::mpsc::sync_channel(options.channel_capacity);
    let (guard, stop) = ShutdownGuard::new();
    let symbol = instrument.symbol.clone();

    runtime.spawn(async move {
        run(symbol, options, sender, stop).await;
    });

    ChannelFeed::new(receiver, guard)
}

async fn run(
    symbol: String,
    mut options: LiveOptions,
    sender: SyncSender<MarketEvent>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let started = std::time::Instant::now();
        let outcome = session(&symbol, &options, &sender, &stop).await;

        if stop.load(Ordering::Relaxed) {
            break;
        }

        let detail = match outcome {
            Ok(()) => "stream closed".to_string(),
            Err(e) => e.to_string(),
        };
        let _ = sender.try_send(MarketEvent::Status {
            ts: Ts::now(),
            connected: false,
            detail,
        });

        if started.elapsed() > std::time::Duration::from_secs(60) {
            options.backoff.reset();
        }
        tokio::time::sleep(options.backoff.next_delay()).await;
    }
}

async fn session(
    symbol: &str,
    options: &LiveOptions,
    sender: &SyncSender<MarketEvent>,
    stop: &Arc<AtomicBool>,
) -> Result<(), LiveError> {
    let (mut socket, _) = tokio_tungstenite::connect_async(WS_SPOT)
        .await
        .map_err(|e| LiveError::WebSocket(e.to_string()))?;

    let subscribe = serde_json::json!({
        "op": "subscribe",
        "args": [trade_topic(symbol), orderbook_topic(symbol, BOOK_DEPTH)],
    });
    socket
        .send(Message::Text(subscribe.to_string()))
        .await
        .map_err(|e| LiveError::WebSocket(e.to_string()))?;

    let _ = sender.try_send(MarketEvent::Status {
        ts: Ts::now(),
        connected: true,
        detail: format!("connected to {symbol}"),
    });

    let mut last_ping = std::time::Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Bybit closes idle connections, so the client pings rather than
        // waiting to be disconnected on a quiet instrument.
        if last_ping.elapsed() > std::time::Duration::from_secs(20) {
            socket
                .send(Message::Text(r#"{"op":"ping"}"#.to_string()))
                .await
                .map_err(|e| LiveError::WebSocket(e.to_string()))?;
            last_ping = std::time::Instant::now();
        }

        let message = tokio::time::timeout(options.read_timeout, socket.next())
            .await
            .map_err(|_| LiveError::ReadTimeout(options.read_timeout))?;

        let Some(message) = message else {
            return Ok(());
        };
        let message = message.map_err(|e| LiveError::WebSocket(e.to_string()))?;

        match message {
            Message::Ping(payload) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|e| LiveError::WebSocket(e.to_string()))?;
            }
            Message::Close(_) => return Ok(()),
            Message::Text(text) => {
                // Subscription acknowledgements and pongs parse as
                // "unhandled", which is expected traffic rather than an error.
                if let Ok(events) = wire::parse_message(&text) {
                    for event in events {
                        let _ = sender.try_send(event);
                    }
                }
            }
            _ => {}
        }
    }
}
