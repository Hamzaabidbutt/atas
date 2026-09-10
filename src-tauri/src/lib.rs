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
use atas_feed::live::{binance, LiveOptions};
use atas_feed::{Feed, SyntheticFeed};
use atas_trading::{OrderId, OrderRequest};
use serde::Deserialize;
use tauri::Emitter;

/// Event name the front end listens on. Must match `transport.ts`.
const EVENT_CHANNEL: &str = "session://event";

/// Append a line to the log file and to stderr.
///
/// A Windows GUI application has no console, so `eprintln!` alone means a user
/// hitting a problem has literally nothing to look at and nothing to send.
/// The file is the only way most of this is ever diagnosable.
fn log(message: &str) {
    eprintln!("{message}");
    if let Some(path) = log_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(file, "[{}] {message}", stamp());
        }
    }
}

/// Where the log lives, per platform.
fn log_path() -> Option<std::path::PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
    }?;
    Some(base.join("ATAS").join("atas.log"))
}

/// Seconds since the epoch. Deliberately not a calendar format: this crate has
/// no date-time dependency and a log line only needs to be orderable.
fn stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Environment variable selecting the data source.
///
/// `ATAS_FEED=binance:BTCUSDT` connects to Binance; anything unset or
/// unrecognised falls back to the offline simulator. An env var rather than a
/// UI control for now because a venue switch has to tear down and rebuild the
/// session, and getting that wrong mid-session is worse than a restart.
const FEED_ENV: &str = "ATAS_FEED";

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
/// Which data source to run against.
enum FeedChoice {
    /// The offline simulator.
    Synthetic,
    /// Binance live market data for a symbol.
    Binance(String),
}

impl FeedChoice {
    /// Read the choice from the environment.
    ///
    /// An unrecognised value falls back to the simulator with a warning rather
    /// than refusing to start: a typo in an env var should not leave a trader
    /// with no application at all.
    fn from_env() -> Self {
        let Ok(raw) = std::env::var(FEED_ENV) else {
            return FeedChoice::Synthetic;
        };
        let raw = raw.trim();

        if raw.eq_ignore_ascii_case("sim") || raw.eq_ignore_ascii_case("synthetic") {
            return FeedChoice::Synthetic;
        }
        if let Some(symbol) = raw.strip_prefix("binance:").or_else(|| {
            raw.eq_ignore_ascii_case("binance").then_some("BTCUSDT")
        }) {
            let symbol = symbol.trim();
            if symbol.is_empty() {
                log(&format!("{FEED_ENV}: no symbol given; using the simulator"));
                return FeedChoice::Synthetic;
            }
            return FeedChoice::Binance(symbol.to_uppercase());
        }

        log(&format!("{FEED_ENV}: unrecognised value {raw:?}; using the simulator"));
        FeedChoice::Synthetic
    }

    /// The instrument this choice trades.
    fn instrument(&self) -> Instrument {
        match self {
            FeedChoice::Synthetic => Instrument::spot(
                "BTCUSDT",
                Venue::Sim,
                Price::parse("0.01").expect("valid tick size"),
                Qty::parse("0.00001").expect("valid quantity step"),
            ),
            FeedChoice::Binance(symbol) => Instrument::spot(
                symbol,
                Venue::Binance,
                // Binance publishes per-symbol filters over REST; until those
                // are fetched, BTCUSDT's real increments are the default.
                Price::parse("0.01").expect("valid tick size"),
                Qty::parse("0.00001").expect("valid quantity step"),
            ),
        }
    }
}

pub fn run() {
    log("--- ATAS starting ---");
    if let Some(path) = log_path() {
        eprintln!("log file: {}", path.display());
    }

    let choice = FeedChoice::from_env();
    let instrument = choice.instrument();

    // Bar rule matched to the demo feed's rate. A one-minute bar against a
    // feed printing every 25ms is 2,400 trades, which spans so many price
    // levels that the footprint can only render as a heatmap. Bar rules and
    // feed rates have to be chosen together.
    let session = Session::new(
        instrument.clone(),
        SessionConfig {
            bar_spec: BarSpec::Tick { count: 60 },
            // A 0.01 increment would put hundreds of rows in a bar spanning a
            // few dollars. Aggregating ticks into rows is a display choice and
            // leaves order prices at the instrument's real increment.
            ticks_per_row: 25,
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

            let mut feed: Box<dyn Feed + Send> = match &choice {
                FeedChoice::Synthetic => {
                    log(&format!("feed: offline simulator (set {FEED_ENV}=binance:BTCUSDT for live)"));
                    // A step of one 0.01 tick would move the market a cent at
                    // a time, which is not what BTCUSDT does and collapses a
                    // bar into a single footprint row. Twenty ticks is twenty
                    // cents a print.
                    Box::new(
                        SyntheticFeed::new(&instrument, 0xA7A5, Price::from_units(95_000))
                            .with_tick_step(20),
                    )
                }
                FeedChoice::Binance(symbol) => {
                    log(&format!("feed: Binance live, {symbol}"));
                    // The runtime has to outlive the connection task, and the
                    // feed runs for the life of the process, so it is leaked
                    // deliberately rather than dropped at the end of setup —
                    // dropping it would abort the socket immediately.
                    let runtime = Box::leak(Box::new(
                        tokio::runtime::Runtime::new().expect("tokio runtime"),
                    ));
                    Box::new(binance::connect(
                        &instrument,
                        LiveOptions::default(),
                        runtime.handle(),
                    ))
                }
            };

            std::thread::spawn(move || {
                let mut errors = 0u32;
                let mut heartbeat = std::time::Instant::now();
                let mut seen = 0u64;

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
                                    // Connection transitions are the single
                                    // most useful thing in a bug report, so
                                    // they go to the log rather than only to
                                    // a chip in the UI.
                                    if let atas_core::MarketEvent::Status {
                                        connected,
                                        detail,
                                        ..
                                    } = &event
                                    {
                                        log(&format!(
                                            "feed {}: {detail}",
                                            if *connected { "connected" } else { "down" }
                                        ));
                                    }
                                    batch.extend(session.on_market_event(&event).iter().cloned());
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    errors += 1;
                                    // Log the first few and then go quiet: a
                                    // permanently broken feed must not fill
                                    // the disk with identical lines.
                                    if errors <= 5 {
                                        log(&format!("feed error: {e}"));
                                    }
                                    continue;
                                }
                            }
                        }
                    }

                    seen += batch.len() as u64;
                    for event in &batch {
                        let _ = handle.emit(EVENT_CHANNEL, event);
                    }

                    // A periodic line proves the pump is alive. Its absence in
                    // a log is as informative as its contents: it separates
                    // "the app froze" from "the feed delivered nothing".
                    if heartbeat.elapsed() >= std::time::Duration::from_secs(30) {
                        log(&format!("pump alive: {seen} events, {errors} errors"));
                        heartbeat = std::time::Instant::now();
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
