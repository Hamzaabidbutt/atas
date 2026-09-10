//! The application session: one instrument, fully wired.
//!
//! Everything the desktop app does to market data happens here, in plain Rust
//! with no Tauri, no threads and no I/O. The shell around it only moves bytes:
//! it hands [`Session::on_market_event`] whatever the feed produced and ships
//! the resulting [`AppEvent`]s to the UI. Keeping the state machine free of
//! the framework is what makes it testable at all — the alternative is logic
//! that can only be exercised by launching a window.

use std::collections::VecDeque;

use atas_core::{Instrument, MarketEvent, OrderBook, Price, Qty, Side, Trade, Ts};
use atas_engine::{Aggregator, Bar, BarSpec, LadderSpec, LadderSpecError, SpecError};
use atas_indicators::{
    BarIndicator, BigTrade, BigTrades, ClusterCriteria, ClusterHit, ClusterSearch, Cvd, Divergence,
    SessionProfile, SpeedOfTape, TradeIndicator, Vwap,
};
use atas_trading::{
    Fill, OrderId, OrderRequest, PaperEngine, RejectReason, SubmitOutcome, TradingConfig,
};
use serde::Serialize;

use crate::dto::*;

/// How the session is configured.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Bar construction rule.
    pub bar_spec: BarSpec,
    /// Instrument ticks per footprint row.
    ///
    /// Independent of the instrument's own increment: BTCUSDT ticks at 0.01,
    /// which puts hundreds of rows in a bar spanning a few dollars. Raising
    /// this makes the ladder legible without quantising order prices.
    pub ticks_per_row: u32,
    /// How many completed bars to keep for the chart.
    pub bar_history: usize,
    /// How many tape rows to keep.
    pub tape_length: usize,
    /// Book depth to publish.
    pub book_depth: usize,
    /// Ratio defining a diagonal imbalance.
    pub imbalance_ratio: f64,
    /// Minimum level volume for an imbalance to count.
    pub imbalance_min_volume: Qty,
    /// Size at or above which a print is flagged as a big trade.
    pub big_trade_threshold: Qty,
    /// Window for speed of tape, in nanoseconds.
    pub speed_window_nanos: i64,
    /// Trading configuration.
    pub trading: TradingConfig,
    /// Scanner criteria. Empty criteria match nothing.
    pub scanner: ClusterCriteria,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            bar_spec: BarSpec::minutes(1),
            ticks_per_row: 1,
            bar_history: 500,
            tape_length: 200,
            book_depth: 20,
            imbalance_ratio: 3.0,
            imbalance_min_volume: Qty::ZERO,
            big_trade_threshold: Qty::from_units(100),
            speed_window_nanos: atas_core::time::NANOS_PER_SEC * 10,
            trading: TradingConfig::default(),
            scanner: ClusterCriteria::new(),
        }
    }
}

/// Why a session could not be built or reconfigured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The bar rule was invalid.
    #[error(transparent)]
    BarSpec(#[from] SpecError),
    /// The footprint row mapping was invalid.
    #[error(transparent)]
    Ladder(#[from] LadderSpecError),
}

/// Something the UI should react to.
///
/// Deliberately coarse. A UI told "the forming bar changed" can redraw the
/// last column; a UI told about every field that moved has to reassemble the
/// state itself, which is how two sources of truth get created.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    /// A bar closed. Carries the finished bar.
    BarClosed(Box<BarDto>),
    /// The forming bar changed.
    BarUpdated(Box<BarDto>),
    /// The book changed.
    BookUpdated(BookDto),
    /// A trade printed.
    TapeRow(TapeRowDto),
    /// A print at or above the big-trade threshold.
    BigTrade {
        /// When, nanoseconds.
        ts: i64,
        /// Price, minor units.
        price: i64,
        /// Size, minor units.
        qty: i64,
        /// `"buy"` or `"sell"`.
        side: &'static str,
    },
    /// The scanner matched a bar.
    ScannerHit {
        /// Bar open time, nanoseconds.
        open_ts: i64,
        /// Bar volume, minor units.
        volume: i64,
        /// Bar delta, minor units.
        delta: i64,
        /// How many stacked-imbalance runs were found.
        stacks: usize,
    },
    /// An order filled.
    Filled(FillDto),
    /// Position or PnL changed.
    PositionChanged(PositionDto),
    /// Feed connection state changed.
    ConnectionChanged {
        /// Whether the feed is connected.
        connected: bool,
        /// Human-readable detail.
        detail: String,
    },
}

/// One instrument's live state.
#[derive(Debug)]
pub struct Session {
    instrument: Instrument,
    config: SessionConfig,
    aggregator: Aggregator,
    book: OrderBook,
    bars: VecDeque<Bar>,
    tape: VecDeque<Trade>,
    cvd: Cvd,
    vwap: Vwap,
    profile: SessionProfile,
    big_trades: BigTrades,
    speed: SpeedOfTape,
    scanner: ClusterSearch,
    engine: PaperEngine,
    events: Vec<AppEvent>,
    last_ts: Ts,
    connected: bool,
    connection_detail: String,
}

impl Session {
    /// Build a session for an instrument.
    pub fn new(instrument: Instrument, config: SessionConfig) -> Result<Self, ConfigError> {
        let ladder = LadderSpec::new(instrument.tick_size, config.ticks_per_row)?;
        let aggregator = Aggregator::with_ladder(ladder, config.bar_spec)?;
        let engine = PaperEngine::new(instrument.clone(), config.trading.clone());
        Ok(Self {
            // The session profile shares the chart's row height, or its POC
            // would mark a price the footprint has no row for.
            profile: SessionProfile::with_spec(ladder),
            big_trades: BigTrades::new(config.big_trade_threshold, 100),
            speed: SpeedOfTape::new(config.speed_window_nanos),
            scanner: ClusterSearch::new(config.scanner.clone()),
            cvd: Cvd::new(),
            vwap: Vwap::new(),
            bars: VecDeque::with_capacity(config.bar_history),
            tape: VecDeque::with_capacity(config.tape_length),
            aggregator,
            book: OrderBook::new(),
            engine,
            events: Vec::new(),
            last_ts: Ts::EPOCH,
            connected: false,
            connection_detail: String::new(),
            instrument,
            config,
        })
    }

    /// The instrument this session tracks.
    pub fn instrument(&self) -> &Instrument {
        &self.instrument
    }

    /// Completed bars, oldest first.
    pub fn bars(&self) -> impl Iterator<Item = &Bar> {
        self.bars.iter()
    }

    /// The bar currently forming.
    pub fn forming_bar(&self) -> Option<&Bar> {
        self.aggregator.current()
    }

    /// The current book.
    pub fn book(&self) -> &OrderBook {
        &self.book
    }

    /// The paper trading engine.
    pub fn engine(&self) -> &PaperEngine {
        &self.engine
    }

    /// Scanner hits so far.
    pub fn scanner_hits(&self) -> &[ClusterHit] {
        self.scanner.hits()
    }

    /// Big trades seen recently.
    pub fn big_trades(&self) -> impl Iterator<Item = &BigTrade> {
        self.big_trades.recent()
    }

    /// Whether the feed reports itself connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// The most recent connection message.
    pub fn connection_detail(&self) -> &str {
        &self.connection_detail
    }

    /// Feed one normalised market event in, returning what the UI should do.
    ///
    /// The returned slice is valid until the next call.
    pub fn on_market_event(&mut self, event: &MarketEvent) -> &[AppEvent] {
        self.events.clear();
        self.last_ts = event.ts();

        match event {
            MarketEvent::Trade(trade) => self.on_trade(trade),
            MarketEvent::BookSnapshot {
                ts,
                bids,
                asks,
                sequence,
            } => {
                self.book
                    .apply_snapshot(*ts, bids.iter().copied(), asks.iter().copied(), *sequence);
                let book = self.book_dto();
                self.events.push(AppEvent::BookUpdated(book));
            }
            MarketEvent::BookDelta {
                ts,
                bids,
                asks,
                sequence,
            } => {
                // A rejected delta is not an error to surface: a stale
                // sequence is the normal consequence of REST and websocket
                // overlapping during startup.
                if self
                    .book
                    .apply_delta(*ts, bids.iter().copied(), asks.iter().copied(), *sequence)
                    .is_ok()
                {
                    let book = self.book_dto();
                    self.events.push(AppEvent::BookUpdated(book));
                }
            }
            MarketEvent::Status {
                connected, detail, ..
            } => {
                self.connected = *connected;
                self.connection_detail = detail.clone();
                if !connected {
                    // Stale depth must never be shown as live.
                    self.book.clear();
                }
                self.events.push(AppEvent::ConnectionChanged {
                    connected: *connected,
                    detail: detail.clone(),
                });
            }
        }
        &self.events
    }

    fn on_trade(&mut self, trade: &Trade) {
        // Tape and trade-level indicators see every print.
        if self.tape.len() == self.config.tape_length {
            self.tape.pop_front();
        }
        self.tape.push_back(*trade);
        self.events.push(AppEvent::TapeRow(TapeRowDto::from(trade)));

        self.speed.update(trade);
        if let Some(big) = self.big_trades.update(trade) {
            self.events.push(AppEvent::BigTrade {
                ts: big.ts.nanos(),
                price: big.price.minor(),
                qty: big.qty.minor(),
                side: big.aggressor.as_str(),
            });
        }

        // Paper fills happen against the same print the chart sees.
        let fills = self.engine.on_trade(trade, &self.book);
        let filled = !fills.is_empty();
        for fill in &fills {
            self.events.push(AppEvent::Filled(FillDto::from(fill)));
        }
        if filled {
            let position = self.position_dto();
            self.events.push(AppEvent::PositionChanged(position));
        }

        // Aggregation last, so a bar closing sees a book and tape already
        // updated by the same trade.
        let closed: Vec<Bar> = self.aggregator.on_trade(trade).to_vec();
        for bar in closed {
            self.cvd.update(&bar);
            self.vwap.update(&bar);
            self.profile.update(&bar);

            if let Some(hit) = self.scanner.scan(&bar) {
                self.events.push(AppEvent::ScannerHit {
                    open_ts: hit.open_ts.nanos(),
                    volume: hit.volume.minor(),
                    delta: hit.delta.minor(),
                    stacks: hit.stacks.len(),
                });
            }

            let dto = self.bar_dto(&bar);
            if self.bars.len() == self.config.bar_history {
                self.bars.pop_front();
            }
            self.bars.push_back(bar);
            self.events.push(AppEvent::BarClosed(Box::new(dto)));
        }

        if let Some(forming) = self.aggregator.current() {
            let dto = BarDto::from_bar(
                forming,
                self.config.imbalance_ratio,
                self.config.imbalance_min_volume,
            );
            self.events.push(AppEvent::BarUpdated(Box::new(dto)));
        }
    }

    fn bar_dto(&self, bar: &Bar) -> BarDto {
        BarDto::from_bar(
            bar,
            self.config.imbalance_ratio,
            self.config.imbalance_min_volume,
        )
    }

    fn book_dto(&self) -> BookDto {
        BookDto {
            bids: self
                .book
                .bids(self.config.book_depth)
                .into_iter()
                .map(|l| LevelDto {
                    price: l.price.minor(),
                    qty: l.qty.minor(),
                })
                .collect(),
            asks: self
                .book
                .asks(self.config.book_depth)
                .into_iter()
                .map(|l| LevelDto {
                    price: l.price.minor(),
                    qty: l.qty.minor(),
                })
                .collect(),
            spread: self.book.spread().map(|s| s.minor()),
            crossed: self.book.is_crossed(),
        }
    }

    fn position_dto(&self) -> PositionDto {
        PositionDto::from_position(self.engine.position(), self.engine.last_price())
    }

    fn indicators_dto(&self) -> IndicatorsDto {
        let vwap = self.vwap.value().flatten();
        let value_area = self.profile.value_area();
        IndicatorsDto {
            cvd: self.cvd.total().minor(),
            divergence: self.cvd.divergence().map(|d| match d {
                Divergence::Bearish => "bearish",
                Divergence::Bullish => "bullish",
            }),
            vwap: vwap.map(|v| v.vwap.minor()),
            vwap_deviation: vwap.map(|v| v.deviation.minor()),
            session_poc: self.profile.poc().map(|p| p.minor()),
            value_area_high: value_area.map(|v| v.high.minor()),
            value_area_low: value_area.map(|v| v.low.minor()),
            speed_of_tape: self.speed.value().unwrap_or(0.0),
        }
    }

    /// A complete frame for the UI.
    ///
    /// Used on connect and on reconnect. Steady-state updates go through
    /// [`AppEvent`]s instead, so a busy instrument does not re-serialise
    /// hundreds of bars on every print.
    pub fn snapshot(&self) -> SnapshotDto {
        let mut bars: Vec<BarDto> = self.bars.iter().map(|b| self.bar_dto(b)).collect();
        if let Some(forming) = self.aggregator.current() {
            bars.push(self.bar_dto(forming));
        }

        SnapshotDto {
            instrument: self.instrument.key(),
            scale: PRICE_SCALE,
            tick_size: self.instrument.tick_size.minor(),
            row_size: self.ladder().row_size().minor(),
            ticks_per_row: self.ladder().ticks_per_row(),
            price_decimals: self.instrument.price_decimals,
            bars,
            book: self.book_dto(),
            tape: self.tape.iter().map(TapeRowDto::from).collect(),
            indicators: self.indicators_dto(),
            position: self.position_dto(),
            orders: self.engine.working_orders().map(OrderDto::from).collect(),
            last_price: self.engine.last_price().map(|p| p.minor()),
            last_ts: self.last_ts.nanos(),
            connected: self.connected,
            connection_detail: self.connection_detail.clone(),
        }
    }

    // --- Trading commands -------------------------------------------------

    /// Submit an order against the current book.
    pub fn submit_order(&mut self, request: OrderRequest) -> Result<SubmitOutcome, RejectReason> {
        let outcome = self.engine.submit(request, &self.book, self.last_ts)?;
        Ok(outcome)
    }

    /// Cancel a working order.
    pub fn cancel_order(&mut self, id: OrderId) -> bool {
        self.engine.cancel(id, self.last_ts)
    }

    /// Cancel every working order.
    pub fn cancel_all_orders(&mut self) -> usize {
        self.engine.cancel_all(self.last_ts)
    }

    /// Cancel everything and close the position at market.
    pub fn flatten(&mut self) -> Result<Vec<Fill>, RejectReason> {
        self.engine.flatten(&self.book, self.last_ts)
    }

    /// Buy at market.
    pub fn buy_market(&mut self, qty: Qty) -> Result<SubmitOutcome, RejectReason> {
        self.submit_order(OrderRequest::market(Side::Buy, qty))
    }

    /// Sell at market.
    pub fn sell_market(&mut self, qty: Qty) -> Result<SubmitOutcome, RejectReason> {
        self.submit_order(OrderRequest::market(Side::Sell, qty))
    }

    /// Place a resting limit order.
    pub fn place_limit(
        &mut self,
        side: Side,
        qty: Qty,
        price: Price,
    ) -> Result<SubmitOutcome, RejectReason> {
        self.submit_order(OrderRequest::limit(side, qty, price))
    }

    // --- Configuration ----------------------------------------------------

    /// Change the bar rule, discarding chart history.
    ///
    /// History is discarded rather than rebuilt because bars under the old
    /// rule cannot be reinterpreted under the new one — only a replay from
    /// stored ticks can do that, and that is the caller's decision to make.
    pub fn set_bar_spec(&mut self, spec: BarSpec) -> Result<(), SpecError> {
        self.aggregator = Aggregator::with_ladder(self.aggregator.ladder(), spec)?;
        self.config.bar_spec = spec;
        self.bars.clear();
        self.cvd.reset();
        self.vwap.reset();
        self.profile.reset();
        Ok(())
    }

    /// Change how many instrument ticks a footprint row spans.
    ///
    /// Chart history is discarded: existing bars were bucketed at the old row
    /// height and re-bucketing them would need the ticks they were built from,
    /// which the bars no longer carry.
    pub fn set_ticks_per_row(&mut self, ticks_per_row: u32) -> Result<(), ConfigError> {
        let ladder = LadderSpec::new(self.instrument.tick_size, ticks_per_row)?;
        self.aggregator = Aggregator::with_ladder(ladder, self.config.bar_spec)?;
        self.config.ticks_per_row = ticks_per_row;
        self.bars.clear();
        self.cvd.reset();
        self.vwap.reset();
        self.profile = SessionProfile::with_spec(ladder);
        Ok(())
    }

    /// The footprint row mapping in use.
    pub fn ladder(&self) -> LadderSpec {
        self.aggregator.ladder()
    }

    /// Replace the scanner criteria and clear previous hits.
    pub fn set_scanner(&mut self, criteria: ClusterCriteria) {
        self.config.scanner = criteria.clone();
        self.scanner = ClusterSearch::new(criteria);
    }

    /// Start a fresh session: clears indicators, chart and tape, and keeps the
    /// position, because a session boundary is not a reason to abandon a live
    /// trade.
    pub fn reset_session(&mut self) {
        self.aggregator.reset();
        self.bars.clear();
        self.tape.clear();
        self.cvd.reset();
        self.vwap.reset();
        self.profile.reset();
        self.speed.reset();
        self.scanner.reset();
    }
}
