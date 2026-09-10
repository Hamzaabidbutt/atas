//! Position and PnL tracking.
//!
//! PnL is reported in the quote currency for spot and linear contracts, and in
//! the base currency for inverse contracts. That distinction is not pedantry:
//! an inverse perpetual's payoff is non-linear in price, so applying the
//! linear formula to one misreports every position — subtly at first, then
//! badly as price moves.

use atas_core::{Instrument, InstrumentKind, Price, Qty, Side, SCALE};
use serde::{Deserialize, Serialize};

/// A net position in one instrument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// Signed size: positive is long, negative is short, zero is flat.
    pub qty: Qty,
    /// Volume-weighted entry price of the open quantity.
    pub avg_price: Price,
    /// PnL banked by closing quantity.
    pub realised: Qty,
    /// Commission paid over the life of the position.
    pub commission: Qty,
    /// How the payoff is computed.
    kind: InstrumentKind,
}

impl Position {
    /// A flat position in an instrument.
    pub fn new(instrument: &Instrument) -> Self {
        Self {
            qty: Qty::ZERO,
            avg_price: Price::ZERO,
            realised: Qty::ZERO,
            commission: Qty::ZERO,
            kind: instrument.kind,
        }
    }

    /// Whether the position is flat.
    #[inline]
    pub fn is_flat(&self) -> bool {
        self.qty.is_zero()
    }

    /// Whether the position is long.
    #[inline]
    pub fn is_long(&self) -> bool {
        self.qty.is_positive()
    }

    /// Whether the position is short.
    #[inline]
    pub fn is_short(&self) -> bool {
        self.qty.is_negative()
    }

    /// Absolute size held.
    #[inline]
    pub fn size(&self) -> Qty {
        self.qty.abs()
    }

    /// Which side the position is on, or `None` when flat.
    pub fn side(&self) -> Option<Side> {
        if self.qty.is_positive() {
            Some(Side::Buy)
        } else if self.qty.is_negative() {
            Some(Side::Sell)
        } else {
            None
        }
    }

    /// Apply a fill, returning the PnL it realised.
    ///
    /// Adding to a position re-averages the entry. Reducing one banks PnL on
    /// the closed quantity and leaves the average alone. A fill large enough
    /// to flip the position closes it fully, banks that, and opens the
    /// remainder at the fill price — the case that a naive implementation
    /// gets wrong by carrying the old average across the flip.
    pub fn apply_fill(&mut self, side: Side, price: Price, qty: Qty, commission: Qty) -> Qty {
        self.commission += commission;
        let signed = Qty::from_minor(qty.minor() * side.signum());

        // Opening from flat, or adding in the same direction.
        if self.qty.is_zero() || self.qty.is_positive() == signed.is_positive() {
            let new_qty = self.qty + signed;
            if !new_qty.is_zero() {
                self.avg_price = weighted_average(
                    self.avg_price,
                    self.qty.abs(),
                    price,
                    qty,
                );
            }
            self.qty = new_qty;
            return Qty::ZERO;
        }

        // Reducing, closing, or flipping.
        let closing = self.qty.abs().min(qty);
        let direction = if self.qty.is_positive() {
            Side::Buy
        } else {
            Side::Sell
        };
        let pnl = self.payoff(direction, self.avg_price, price, closing);
        self.realised += pnl;

        let remaining_after_close = qty - closing;
        self.qty += signed;

        if self.qty.is_zero() {
            self.avg_price = Price::ZERO;
        } else if remaining_after_close.is_positive() {
            // Flipped: the new position starts fresh at this fill's price.
            self.avg_price = price;
        }
        pnl
    }

    /// Unrealised PnL if the position were marked at `mark`.
    pub fn unrealised(&self, mark: Price) -> Qty {
        match self.side() {
            None => Qty::ZERO,
            Some(side) => self.payoff(side, self.avg_price, mark, self.qty.abs()),
        }
    }

    /// Realised plus unrealised, net of commission.
    pub fn total_pnl(&self, mark: Price) -> Qty {
        self.realised + self.unrealised(mark) - self.commission
    }

    /// Payoff of closing `qty` of a `side` position opened at `entry`.
    fn payoff(&self, side: Side, entry: Price, exit: Price, qty: Qty) -> Qty {
        match self.kind {
            InstrumentKind::Spot | InstrumentKind::LinearPerp => {
                // (exit − entry) × qty, in quote currency.
                let move_minor = (exit.minor() - entry.minor()) as i128 * side.signum() as i128;
                let pnl = move_minor * qty.minor() as i128 / SCALE as i128;
                Qty::from_minor(pnl.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
            }
            InstrumentKind::InversePerp => {
                // qty × (1/entry − 1/exit), in base currency. Non-linear in
                // price, which is exactly why it cannot share the branch above.
                if entry.minor() == 0 || exit.minor() == 0 {
                    return Qty::ZERO;
                }
                let scale = SCALE as i128;
                let inv_entry = scale * scale / entry.minor() as i128;
                let inv_exit = scale * scale / exit.minor() as i128;
                let diff = (inv_entry - inv_exit) * side.signum() as i128;
                let pnl = diff * qty.minor() as i128 / scale;
                Qty::from_minor(pnl.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
            }
        }
    }
}

/// Volume-weighted average of two prices.
fn weighted_average(price_a: Price, qty_a: Qty, price_b: Price, qty_b: Qty) -> Price {
    let total = qty_a.minor() as i128 + qty_b.minor() as i128;
    if total <= 0 {
        return price_b;
    }
    let weighted = price_a.minor() as i128 * qty_a.minor() as i128
        + price_b.minor() as i128 * qty_b.minor() as i128;
    Price::from_minor((weighted / total) as i64)
}
