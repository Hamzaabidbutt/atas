//! Binance live connection.
//!
//! Implements the depth handshake Binance specifies: open the stream, buffer
//! updates, fetch a REST snapshot, discard what the snapshot already covers,
//! and verify the remainder joins on. The sequencing itself is
//! [`DepthSynchroniser`], which is tested exhaustively; this file supplies it
//! with bytes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use atas_core::{Instrument, MarketEvent, Ts};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use crate::binance::{self as wire, agg_trade_stream, depth_stream};
use crate::live::{ChannelFeed, LiveError, LiveOptions, ShutdownGuard};
use crate::sync::{DepthSynchroniser, SyncAction};

const WS_BASE: &str = "wss://stream.binance.com:9443/stream?streams=";
const REST_BASE: &str = "https://api.binance.com";

/// Connect to Binance for one instrument.
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

/// Reconnect loop. Each pass is one connection attempt, from handshake to
/// failure; the loop itself owns pacing and shutdown.
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
            detail: detail.clone(),
        });

        // A connection that survived a while was healthy; treat the next
        // failure as the first one rather than compounding an old streak.
        if started.elapsed() > std::time::Duration::from_secs(60) {
            options.backoff.reset();
        }
        tokio::time::sleep(options.backoff.next_delay()).await;
    }
}

/// One connection: subscribe, synchronise depth, then pump messages.
async fn session(
    symbol: &str,
    options: &LiveOptions,
    sender: &SyncSender<MarketEvent>,
    stop: &Arc<AtomicBool>,
) -> Result<(), LiveError> {
    let lower = symbol.to_lowercase();
    let url = format!(
        "{WS_BASE}{}/{}",
        agg_trade_stream(&lower),
        depth_stream(&lower)
    );

    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| LiveError::WebSocket(e.to_string()))?;

    let _ = sender.try_send(MarketEvent::Status {
        ts: Ts::now(),
        connected: true,
        detail: format!("connected to {symbol}"),
    });

    let mut sync: DepthSynchroniser<MarketEvent> = DepthSynchroniser::new();
    // The snapshot is requested only after the stream is open, so no update
    // between the two can be missed.
    let mut snapshot_pending = true;

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        let message = tokio::time::timeout(options.read_timeout, socket.next())
            .await
            .map_err(|_| LiveError::ReadTimeout(options.read_timeout))?;

        let Some(message) = message else {
            return Ok(()); // stream ended
        };
        let message = message.map_err(|e| LiveError::WebSocket(e.to_string()))?;

        match message {
            // tungstenite answers pings itself, but a venue that only ever
            // pings would otherwise never trip the read timeout.
            Message::Ping(payload) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|e| LiveError::WebSocket(e.to_string()))?;
                continue;
            }
            Message::Close(_) => return Ok(()),
            Message::Text(text) => {
                handle_text(&text, sender, &mut sync, &mut snapshot_pending, symbol, options)
                    .await?;
            }
            _ => continue,
        }
    }
}

/// Route one text frame.
async fn handle_text(
    text: &str,
    sender: &SyncSender<MarketEvent>,
    sync: &mut DepthSynchroniser<MarketEvent>,
    snapshot_pending: &mut bool,
    symbol: &str,
    options: &LiveOptions,
) -> Result<(), LiveError> {
    let value: Value = serde_json::from_str(text).map_err(crate::FeedError::from)?;
    let payload = wire::unwrap_stream(&value);
    let kind = payload.get("e").and_then(Value::as_str).unwrap_or_default();

    match kind {
        "aggTrade" | "trade" => {
            let trade = wire::parse_trade(payload)?;
            // A full channel means the consumer has stalled. Dropping the
            // newest print is better than blocking the socket read loop and
            // letting the venue disconnect us for being slow.
            let _ = sender.try_send(MarketEvent::Trade(trade));
        }
        "depthUpdate" => {
            let first = wire::depth_update_first_id(&value)?;
            let event = wire::parse_depth_update(payload)?;
            let final_id = match &event {
                MarketEvent::BookDelta { sequence, .. } => *sequence,
                _ => 0,
            };

            let (action, applied) = sync.on_delta(first, final_id, event);
            match action {
                SyncAction::Apply => {
                    if let Some(event) = applied {
                        let _ = sender.try_send(event);
                    }
                }
                SyncAction::Discard => {}
                SyncAction::Buffer => {
                    if *snapshot_pending {
                        *snapshot_pending = false;
                        fetch_and_apply_snapshot(symbol, options, sender, sync).await?;
                    }
                }
                SyncAction::Resync => {
                    sync.restart();
                    *snapshot_pending = true;
                    let _ = sender.try_send(MarketEvent::Status {
                        ts: Ts::now(),
                        connected: true,
                        detail: "depth gap; resynchronising".to_string(),
                    });
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Fetch the REST snapshot and release the buffered updates behind it.
async fn fetch_and_apply_snapshot(
    symbol: &str,
    options: &LiveOptions,
    sender: &SyncSender<MarketEvent>,
    sync: &mut DepthSynchroniser<MarketEvent>,
) -> Result<(), LiveError> {
    let url = format!(
        "{REST_BASE}/api/v3/depth?symbol={}&limit={}",
        symbol.to_uppercase(),
        options.snapshot_depth
    );
    let body = reqwest::get(&url)
        .await
        .map_err(|e| LiveError::Http(e.to_string()))?
        .text()
        .await
        .map_err(|e| LiveError::Http(e.to_string()))?;

    let snapshot = wire::parse_depth_snapshot(&body, Ts::now())?;
    let MarketEvent::BookSnapshot { sequence, .. } = &snapshot else {
        return Ok(());
    };
    let sequence = *sequence;

    match sync.on_snapshot(sequence) {
        Ok(pending) => {
            let _ = sender.try_send(snapshot);
            for event in pending {
                let _ = sender.try_send(event);
            }
        }
        Err(_) => {
            // The snapshot predates the buffered updates. Retrying with a
            // fresh one is correct; applying it would leave a hole.
            sync.restart();
        }
    }
    Ok(())
}
