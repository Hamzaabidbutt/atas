//! The desktop shell.
//!
//! Deliberately thin. Everything this binary does to market data happens in
//! `atas-app`: the session state machine and the pump loop are both plain Rust
//! with tests. What is left here is the part that genuinely needs a window —
//! owning the session behind a lock, running the pump on a background thread,
//! forwarding events to the webview, and translating UI commands into calls on
//! the session.
//!
//! **This has not been run.** It was written in an environment with no
//! webkit2gtk, where a Tauri binary cannot be compiled or launched. The layers
//! underneath it are tested; this file is not, and the first window it opens
//! will be on someone else's machine.

use std::sync::{Arc, Mutex};

use atas_app::{AppEvent, Session, SessionConfig, SnapshotDto};
use atas_core::{Instrument, Price, Qty, Side, Venue};
use atas_engine::BarSpec;
use atas_feed::{Feed, SyntheticFeed};
use atas_trading::{OrderId, OrderRequest};
use serde::Deserialize;
use tauri::Emitter;

/// Event name the front end listens on. Must match `transport.ts`.
const EVENT_CHANNEL: &str = "session://event";

/// How many market events one pump pass may process.
///
/// Bounded so the pump thread yields regularly rather than monopolising a core
/// during a burst, and so the lock on the session is released often enough
/// that UI commands are not starved behind a backlog.
const PUMP_BUDGET: usize = 512;

/// Shared application state.
pub struct AppState {
    session: Arc<Mutex<Session>>,
}

impl AppState {
    /// Lock the session, recovering from a poisoned lock.
    ///
    /// A panic in one command must not brick every later one: the session's
    /// invariants are re-established on the next market event, so continuing
    /// is better than refusing to work until restart.
    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Parse a decimal string from the UI into a quantity.
fn parse_qty(raw: &str) -> Result<Qty, String> {
    Qty::parse(raw).map_err(|e| format!("invalid quantity {raw:?}: {e}"))
}

/// Parse a decimal string from the UI into a price.
fn parse_price(raw: &str) -> Result<Price, String> {
    Price::parse(raw).map_err(|e| format!("invalid price {raw:?}: {e}"))
}

// --- Commands -------------------------------------------------------------

/// A full frame for first paint and after a reconnect.
#[tauri::command]
fn session_snapshot(state: tauri::State<'_, AppState>) -> SnapshotDto {
    state.session().snapshot()
}

/// Buy at market.
///
/// Sizes arrive as decimal strings rather than numbers: routing an order
/// quantity through a float is exactly the mistake fixed point exists to
/// prevent, and the boundary is the easiest place to make it by accident.
#[tauri::command]
fn buy_market(state: tauri::State<'_, AppState>, qty: String) -> Result<(), String> {
    let qty = parse_qty(&qty)?;
    state
        .session()
        .buy_market(qty)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Sell at market.
#[tauri::command]
fn sell_market(state: tauri::State<'_, AppState>, qty: String) -> Result<(), String> {
    let qty = parse_qty(&qty)?;
    state
        .session()
        .sell_market(qty)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Place a resting limit order.
#[tauri::command]
fn place_limit(
    state: tauri::State<'_, AppState>,
    side: String,
    qty: String,
    price: String,
) -> Result<(), String> {
    let side = match side.as_str() {
        "buy" => Side::Buy,
        "sell" => Side::Sell,
        other => return Err(format!("unknown side {other:?}")),
    };
    let qty = parse_qty(&qty)?;
    let price = parse_price(&price)?;

    state
        .session()
        .submit_order(OrderRequest::limit(side, qty, price))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Cancel one working order.
#[tauri::command]
fn cancel_order(state: tauri::State<'_, AppState>, id: u64) -> bool {
    state.session().cancel_order(OrderId(id))
}

/// Cancel every working order.
#[tauri::command]
fn cancel_all(state: tauri::State<'_, AppState>) -> usize {
    state.session().cancel_all_orders()
}

/// Cancel everything and close the position at market.
#[tauri::command]
fn flatten(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    state
        .session()
        .flatten()
        .map(|fills| fills.len())
        .map_err(|e| e.to_string())
}

/// Bar rule selection from the UI.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BarSpecRequest {
    /// Time bars of a given length in seconds.
    Time {
        /// Bar length.
        seconds: i64,
    },
    /// A fixed number of trades per bar.
    Tick {
        /// Trades per bar.
        count: u32,
    },
    /// A volume threshold per bar.
    Volume {
        /// Threshold as a decimal string.
        threshold: String,
    },
    /// A range in ticks.
    Range {
        /// Bar range.
        ticks: i64,
    },
    /// An absolute delta threshold.
    Delta {
        /// Threshold as a decimal string.
        threshold: String,
    },
}

/// Change the bar rule. Chart history is discarded; see `Session::set_bar_spec`.
#[tauri::command]
fn set_bar_spec(state: tauri::State<'_, AppState>, spec: BarSpecRequest) -> Result<(), String> {
    let spec = match spec {
        BarSpecRequest::Time { seconds } => BarSpec::seconds(seconds),
        BarSpecRequest::Tick { count } => BarSpec::Tick { count },
        BarSpecRequest::Volume { threshold } => BarSpec::Volume {
            threshold: parse_qty(&threshold)?,
        },
        BarSpecRequest::Range { ticks } => BarSpec::Range { ticks },
        BarSpecRequest::Delta { threshold } => BarSpec::Delta {
            threshold: parse_qty(&threshold)?,
        },
    };
    state
        .session()
        .set_bar_spec(spec)
        .map_err(|e| e.to_string())
}

// --- Entry point ----------------------------------------------------------

/// Build and run the desktop application.
///
/// # Panics
///
/// Panics if the session cannot be constructed or the Tauri runtime fails to
/// start — both are unrecoverable at launch, and a window that opens without a
/// working session would be worse than no window.
pub fn run() {
    // The tick size here is the *footprint row* size, not necessarily the
    // venue's minimum increment. BTCUSDT ticks at 0.01, which at a 95,000
    // price puts ~150 rows in a single bar — far too many to read, and every
    // real platform aggregates ticks per level for exactly this reason.
    // Until that aggregation exists (see the README), the shell picks a row
    // size that produces a legible ladder.
    let instrument = Instrument::spot(
        "BTCUSDT",
        Venue::Sim,
        Price::parse("0.5").expect("valid tick size"),
        Qty::parse("0.00001").expect("valid quantity step"),
    );

    // Bar rule matched to the demo feed's rate. A one-minute bar against a
    // feed printing every 25ms is 2,400 trades, which spans so many price
    // levels that the footprint can only render as a heatmap. Bar rules and
    // feed rates have to be chosen together.
    let session = Session::new(
        instrument.clone(),
        SessionConfig {
            bar_spec: BarSpec::Tick { count: 60 },
            ..SessionConfig::default()
        },
    )
    .expect("the bar spec must be valid");
    let session = Arc::new(Mutex::new(session));

    tauri::Builder::default()
        .manage(AppState {
            session: Arc::clone(&session),
        })
        .invoke_handler(tauri::generate_handler![
            session_snapshot,
            buy_market,
            sell_market,
            place_limit,
            cancel_order,
            cancel_all,
            flatten,
            set_bar_spec,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let session = Arc::clone(&session);

            // Until a venue is selected in the UI, the synthetic feed keeps
            // the app usable offline. Swapping in `atas_feed::live::binance`
            // is a one-line change here.
            let mut feed = SyntheticFeed::new(&instrument, 0xA7A5, Price::from_units(95_000));

            std::thread::spawn(move || {
                loop {
                    let mut batch: Vec<AppEvent> = Vec::new();
                    {
                        // The lock is held only while draining a bounded
                        // batch, so UI commands are never queued behind a
                        // long burst of market data.
                        let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
                        for _ in 0..PUMP_BUDGET {
                            match feed.next_event() {
                                Ok(Some(event)) => {
                                    batch.extend(session.on_market_event(&event).iter().cloned());
                                }
                                Ok(None) => break,
                                Err(_) => continue,
                            }
                        }
                    }

                    for event in &batch {
                        let _ = handle.emit(EVENT_CHANNEL, event);
                    }

                    // Yield between passes. Without this a quiet feed would
                    // spin a core doing nothing.
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the Tauri application");
}
