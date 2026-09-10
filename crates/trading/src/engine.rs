//! The paper matching engine.
//!
//! # Fill realism
//!
//! A paper engine is only useful if it is pessimistic in the places a real one
//! would be. Two choices matter more than everything else here:
//!
//! **Market orders walk the book.** Taking the whole size at the touch price
//! ignores the cost of clearing depth, which is most of the cost of trading
//! size. This engine consumes level by level and reports the volume-weighted
//! result, so a size too large for the book shows up as the slippage it is.
//!
//! **Resting limit orders need the market to trade *through* them.** A limit
//! buy at 100 does not fill because a trade printed at 100 — the trader was
//! behind everyone already queued at that price. Without a real queue model,
//! the honest assumption is the conservative one: fill only once a trade
//! prints strictly better than the limit. That understates fills slightly,
//! which is the correct direction to be wrong in.

use std::collections::HashMap;

use atas_core::{Instrument, OrderBook, Price, Qty, Side, Trade, Ts};

use crate::order::{
    Fill, Order, OrderId, OrderRequest, OrderStatus, OrderType, RejectReason, TimeInForce,
};
use crate::position::Position;

/// Engine configuration.
#[derive(Debug, Clone)]
pub struct TradingConfig {
    /// Commission per unit of notional, as a fraction (0.0004 is 4 bps).
    pub commission_rate: f64,
    /// Largest absolute position allowed. `None` for unlimited.
    pub max_position: Option<Qty>,
    /// Largest number of simultaneously working orders.
    pub max_working_orders: usize,
    /// Whether market orders may walk past the top of book.
    ///
    /// Turning this off rejects any market order the touch cannot fill, which
    /// models a venue that refuses to sweep.
    pub allow_book_walk: bool,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            commission_rate: 0.0004,
            max_position: None,
            max_working_orders: 256,
            allow_book_walk: true,
        }
    }
}

/// What happened when an order was submitted.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmitOutcome {
    /// The order as the engine recorded it.
    pub order: Order,
    /// Fills produced immediately.
    pub fills: Vec<Fill>,
}

/// Simulated matching against live market data.
#[derive(Debug)]
pub struct PaperEngine {
    instrument: Instrument,
    config: TradingConfig,
    orders: HashMap<OrderId, Order>,
    working: Vec<OrderId>,
    position: Position,
    fills: Vec<Fill>,
    next_id: u64,
    last_price: Option<Price>,
    now: Ts,
}

impl PaperEngine {
    /// An engine for one instrument.
    pub fn new(instrument: Instrument, config: TradingConfig) -> Self {
        let position = Position::new(&instrument);
        Self {
            instrument,
            config,
            orders: HashMap::new(),
            working: Vec::new(),
            position,
            fills: Vec::new(),
            next_id: 1,
            last_price: None,
            now: Ts::EPOCH,
        }
    }

    /// The instrument being traded.
    pub fn instrument(&self) -> &Instrument {
        &self.instrument
    }

    /// The current position.
    pub fn position(&self) -> &Position {
        &self.position
    }

    /// Every fill so far.
    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }

    /// An order by id.
    pub fn order(&self, id: OrderId) -> Option<&Order> {
        self.orders.get(&id)
    }

    /// Orders that can still trade.
    pub fn working_orders(&self) -> impl Iterator<Item = &Order> {
        self.working.iter().filter_map(|id| self.orders.get(id))
    }

    /// Last traded price seen.
    pub fn last_price(&self) -> Option<Price> {
        self.last_price
    }

    /// Unrealised PnL at the last traded price.
    pub fn unrealised(&self) -> Qty {
        self.last_price
            .map(|p| self.position.unrealised(p))
            .unwrap_or(Qty::ZERO)
    }

    /// Realised plus unrealised PnL, net of commission.
    pub fn total_pnl(&self) -> Qty {
        match self.last_price {
            Some(p) => self.position.total_pnl(p),
            None => self.position.realised - self.position.commission,
        }
    }

    fn commission_on(&self, price: Price, qty: Qty) -> Qty {
        if self.config.commission_rate <= 0.0 {
            return Qty::ZERO;
        }
        let notional = price.notional(qty);
        Qty::from_minor((notional.minor() as f64 * self.config.commission_rate) as i64)
    }

    /// Submit an order, matching it against `book` where possible.
    pub fn submit(
        &mut self,
        request: OrderRequest,
        book: &OrderBook,
        now: Ts,
    ) -> Result<SubmitOutcome, RejectReason> {
        self.now = now;
        self.validate(&request)?;

        let id = OrderId(self.next_id);
        self.next_id += 1;

        let triggers = request.kind.trigger_price().is_some();
        let mut order = Order {
            id,
            side: request.side,
            kind: request.kind,
            qty: request.qty,
            filled: Qty::ZERO,
            avg_fill_price: None,
            status: if triggers {
                OrderStatus::Pending
            } else {
                OrderStatus::Working
            },
            tif: request.tif,
            created: now,
            closed: None,
            oco_group: request.oco_group,
            reject_reason: None,
        };

        // Plan before applying. Fill-or-kill has to know the whole outcome
        // before any of it lands, or it is not fill-or-kill.
        let plan: Vec<(Price, Qty)> = match order.kind {
            OrderType::Market => self.plan_market_fill(order.side, order.qty, book)?,
            // A marketable limit crosses the spread, so it takes liquidity
            // immediately rather than resting.
            OrderType::Limit { .. } => self.marketable_limit_fill(&order, book).unwrap_or_default(),
            _ => Vec::new(),
        };

        let mut fills = Vec::new();
        let immediate = matches!(order.kind, OrderType::Market | OrderType::Limit { .. });
        if immediate {
            let planned: Qty = plan.iter().map(|(_, q)| *q).sum();
            if order.tif == TimeInForce::Fok && planned < order.qty {
                // Killed: nothing is applied, so there is nothing to unwind.
                order.status = OrderStatus::Cancelled;
                order.closed = Some(now);
            } else {
                for (price, qty) in plan {
                    fills.push(self.record_fill(&mut order, price, qty, now));
                }
                self.finalise_immediate(&mut order, now);
            }
        }

        if order.status.is_live() {
            self.working.push(id);
        }
        if !fills.is_empty() {
            self.cancel_oco_siblings(id, order.oco_group, now);
        }

        self.orders.insert(id, order.clone());
        Ok(SubmitOutcome { order, fills })
    }

    fn validate(&self, request: &OrderRequest) -> Result<(), RejectReason> {
        if !request.qty.is_positive() {
            return Err(RejectReason::NonPositiveQty);
        }
        for price in [request.kind.limit_price(), request.kind.trigger_price()]
            .into_iter()
            .flatten()
        {
            if !price.is_positive() {
                return Err(RejectReason::NonPositivePrice);
            }
        }
        if self.working.len() >= self.config.max_working_orders {
            return Err(RejectReason::TooManyOrders {
                limit: self.config.max_working_orders,
            });
        }
        if let Some(limit) = self.config.max_position {
            // Only additions to the position can breach the limit; an order
            // that reduces exposure must always be allowed through, or a
            // trader cannot get out of a position that is already too large.
            let signed = Qty::from_minor(request.qty.minor() * request.side.signum());
            let projected = self.position.qty + signed;
            if projected.abs() > limit && projected.abs() > self.position.qty.abs() {
                return Err(RejectReason::PositionLimit { limit });
            }
        }
        Ok(())
    }

    /// Walk the book to price a market order, without mutating anything.
    fn plan_market_fill(
        &self,
        side: Side,
        qty: Qty,
        book: &OrderBook,
    ) -> Result<Vec<(Price, Qty)>, RejectReason> {
        let levels = match side {
            Side::Buy => book.asks(usize::MAX),
            Side::Sell => book.bids(usize::MAX),
        };
        if levels.is_empty() {
            return Err(RejectReason::NoMarket);
        }

        let mut remaining = qty;
        let mut plan = Vec::new();
        for level in levels {
            if !remaining.is_positive() {
                break;
            }
            let take = remaining.min(level.qty);
            plan.push((level.price, take));
            remaining -= take;
            if !self.config.allow_book_walk {
                break;
            }
        }

        if remaining.is_positive() && !self.config.allow_book_walk {
            return Err(RejectReason::NoMarket);
        }
        Ok(plan)
    }

    /// Immediate fill for a limit order that crosses the spread.
    fn marketable_limit_fill(&self, order: &Order, book: &OrderBook) -> Option<Vec<(Price, Qty)>> {
        let limit = order.kind.limit_price()?;
        let levels = match order.side {
            Side::Buy => book.asks(usize::MAX),
            Side::Sell => book.bids(usize::MAX),
        };

        let mut remaining = order.qty;
        let mut plan = Vec::new();
        for level in levels {
            let crosses = match order.side {
                Side::Buy => level.price <= limit,
                Side::Sell => level.price >= limit,
            };
            if !crosses || !remaining.is_positive() {
                break;
            }
            let take = remaining.min(level.qty);
            plan.push((level.price, take));
            remaining -= take;
        }
        (!plan.is_empty()).then_some(plan)
    }

    /// Settle an order that had its chance to fill immediately.
    fn finalise_immediate(&mut self, order: &mut Order, now: Ts) {
        if order.is_complete() {
            order.status = OrderStatus::Filled;
            order.closed = Some(now);
            return;
        }
        match order.tif {
            // Fill-or-kill never reaches here holding a partial fill: `submit`
            // checks the plan against the full quantity before applying any of
            // it, so an incomplete FOK is cancelled with nothing applied.
            TimeInForce::Fok | TimeInForce::Ioc => {
                // Whatever filled, filled; the remainder does not rest.
                order.status = OrderStatus::Cancelled;
                order.closed = Some(now);
            }
            TimeInForce::Gtc => {
                if matches!(order.kind, OrderType::Market) {
                    // A market order that could not be fully filled has no
                    // book left to rest against.
                    order.status = if order.filled.is_positive() {
                        OrderStatus::Filled
                    } else {
                        OrderStatus::Cancelled
                    };
                    order.closed = Some(now);
                } else if order.filled.is_positive() {
                    order.status = OrderStatus::PartiallyFilled;
                }
            }
        }
    }

    /// Apply a fill to an order and to the position.
    fn record_fill(&mut self, order: &mut Order, price: Price, qty: Qty, now: Ts) -> Fill {
        let commission = self.commission_on(price, qty);

        order.avg_fill_price = Some(match order.avg_fill_price {
            None => price,
            Some(previous) => {
                let total = order.filled + qty;
                let weighted = previous.minor() as i128 * order.filled.minor() as i128
                    + price.minor() as i128 * qty.minor() as i128;
                Price::from_minor((weighted / total.minor() as i128) as i64)
            }
        });
        order.filled += qty;

        self.position.apply_fill(order.side, price, qty, commission);
        self.last_price = Some(price);

        let fill = Fill {
            order_id: order.id,
            ts: now,
            price,
            qty,
            side: order.side,
            commission,
        };
        self.fills.push(fill);
        fill
    }

    /// Feed a trade in: triggers stops and fills resting limits.
    ///
    /// Returns the fills the trade caused.
    pub fn on_trade(&mut self, trade: &Trade, book: &OrderBook) -> Vec<Fill> {
        self.now = trade.ts;
        self.last_price = Some(trade.price);

        let mut fills = Vec::new();
        let candidates: Vec<OrderId> = self.working.clone();

        for id in candidates {
            let Some(mut order) = self.orders.get(&id).cloned() else {
                continue;
            };
            if !order.status.is_live() {
                continue;
            }

            // Stops trigger when the market trades at or through them.
            if order.status == OrderStatus::Pending {
                let Some(trigger) = order.kind.trigger_price() else {
                    continue;
                };
                let triggered = match order.side {
                    Side::Buy => trade.price >= trigger,
                    Side::Sell => trade.price <= trigger,
                };
                if !triggered {
                    continue;
                }
                order.status = OrderStatus::Working;

                if matches!(order.kind, OrderType::Stop { .. }) {
                    if let Ok(plan) = self.plan_market_fill(order.side, order.remaining(), book) {
                        for (price, qty) in plan {
                            fills.push(self.record_fill(&mut order, price, qty, trade.ts));
                        }
                    }
                    if order.is_complete() {
                        order.status = OrderStatus::Filled;
                        order.closed = Some(trade.ts);
                    }
                    self.orders.insert(id, order.clone());
                    if order.status.is_done() {
                        self.cancel_oco_siblings(id, order.oco_group, trade.ts);
                    }
                    continue;
                }
                self.orders.insert(id, order.clone());
            }

            // Resting limits fill only once the market trades strictly through
            // them — see the module docs on queue position.
            if let Some(limit) = order.kind.limit_price() {
                let through = match order.side {
                    Side::Buy => trade.price < limit,
                    Side::Sell => trade.price > limit,
                };
                if !through {
                    continue;
                }
                // Fill at the limit price: the trader's resting order cannot
                // have been improved by a trade that happened past it.
                let take = order.remaining().min(trade.qty);
                if take.is_positive() {
                    fills.push(self.record_fill(&mut order, limit, take, trade.ts));
                }
                if order.is_complete() {
                    order.status = OrderStatus::Filled;
                    order.closed = Some(trade.ts);
                } else if order.filled.is_positive() {
                    order.status = OrderStatus::PartiallyFilled;
                }
                self.orders.insert(id, order.clone());
                if order.status.is_done() {
                    self.cancel_oco_siblings(id, order.oco_group, trade.ts);
                }
            }
        }

        self.prune_working();
        fills
    }

    /// Cancel a working order.
    pub fn cancel(&mut self, id: OrderId, now: Ts) -> bool {
        let Some(order) = self.orders.get_mut(&id) else {
            return false;
        };
        if order.status.is_done() {
            return false;
        }
        order.status = OrderStatus::Cancelled;
        order.closed = Some(now);
        self.prune_working();
        true
    }

    /// Cancel every working order, returning how many were cancelled.
    pub fn cancel_all(&mut self, now: Ts) -> usize {
        let ids: Vec<OrderId> = self.working.clone();
        let mut cancelled = 0;
        for id in ids {
            if self.cancel(id, now) {
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Cancel working orders and close the position at market.
    pub fn flatten(&mut self, book: &OrderBook, now: Ts) -> Result<Vec<Fill>, RejectReason> {
        self.cancel_all(now);
        let size = self.position.size();
        if !size.is_positive() {
            return Ok(Vec::new());
        }
        let side = match self.position.side() {
            Some(Side::Buy) => Side::Sell,
            Some(Side::Sell) => Side::Buy,
            None => return Ok(Vec::new()),
        };
        let outcome = self.submit(OrderRequest::market(side, size), book, now)?;
        Ok(outcome.fills)
    }

    /// Cancel the other members of an OCO group.
    fn cancel_oco_siblings(&mut self, filled: OrderId, group: Option<u64>, now: Ts) {
        let Some(group) = group else { return };
        let siblings: Vec<OrderId> = self
            .working
            .iter()
            .copied()
            .filter(|id| *id != filled)
            .filter(|id| {
                self.orders
                    .get(id)
                    .is_some_and(|o| o.oco_group == Some(group) && o.status.is_live())
            })
            .collect();
        for id in siblings {
            self.cancel(id, now);
        }
    }

    fn prune_working(&mut self) {
        let orders = &self.orders;
        self.working
            .retain(|id| orders.get(id).is_some_and(|o| o.status.is_live()));
    }
}
