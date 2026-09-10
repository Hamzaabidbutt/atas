//! Instrument definitions: the contract metadata every price calculation needs.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fixed::{Price, Qty};

/// A trading venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Venue {
    /// Binance spot and USD-M futures.
    Binance,
    /// Bybit spot and perpetuals.
    Bybit,
    /// The built-in simulator and replay feeds.
    Sim,
}

impl Venue {
    /// Stable lower-case identifier, used in storage paths and config files.
    pub const fn as_str(self) -> &'static str {
        match self {
            Venue::Binance => "binance",
            Venue::Bybit => "bybit",
            Venue::Sim => "sim",
        }
    }
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of contract an instrument is.
///
/// This drives PnL: a linear perpetual settles in the quote currency, an
/// inverse contract settles in the base, and getting it wrong silently
/// misreports every position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentKind {
    /// Spot: PnL is `(exit - entry) * qty` in quote currency.
    Spot,
    /// Linear perpetual or future, quote-settled. Same PnL formula as spot.
    LinearPerp,
    /// Inverse perpetual, base-settled: PnL is `qty * (1/entry - 1/exit)`.
    InversePerp,
}

impl InstrumentKind {
    /// Whether PnL for this contract settles in the quote currency.
    pub const fn is_quote_settled(self) -> bool {
        matches!(self, InstrumentKind::Spot | InstrumentKind::LinearPerp)
    }
}

/// A tradable instrument and the metadata needed to price and size it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instrument {
    /// Venue-native symbol, e.g. `BTCUSDT`.
    pub symbol: String,
    /// Where it trades.
    pub venue: Venue,
    /// Contract type, which selects the PnL formula.
    pub kind: InstrumentKind,
    /// Minimum price increment. Cluster ladder rows are one tick apart.
    pub tick_size: Price,
    /// Minimum quantity increment.
    pub qty_step: Qty,
    /// Decimal places to show for prices.
    pub price_decimals: u32,
    /// Decimal places to show for quantities.
    pub qty_decimals: u32,
    /// Units of the underlying per contract. `1` for spot and linear perps.
    pub contract_multiplier: i64,
}

impl Instrument {
    /// A spot instrument with the given tick size and quantity step.
    pub fn spot(
        symbol: impl Into<String>,
        venue: Venue,
        tick_size: Price,
        qty_step: Qty,
    ) -> Self {
        let tick_size_val = tick_size;
        let qty_step_val = qty_step;
        Self {
            symbol: symbol.into(),
            venue,
            kind: InstrumentKind::Spot,
            tick_size: tick_size_val,
            qty_step: qty_step_val,
            price_decimals: decimals_for(tick_size_val.minor()),
            qty_decimals: decimals_for(qty_step_val.minor()),
            contract_multiplier: 1,
        }
    }

    /// Stable key for storage paths and workspace files, e.g. `binance:BTCUSDT`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.venue.as_str(), self.symbol)
    }

    /// Ladder row index for a price, one row per tick.
    #[inline]
    pub fn tick_index(&self, price: Price) -> i64 {
        price.tick_index(self.tick_size)
    }

    /// Price at a ladder row index.
    #[inline]
    pub fn price_at(&self, tick_index: i64) -> Price {
        Price::from_tick_index(tick_index, self.tick_size)
    }

    /// Format a price for display with this instrument's precision.
    pub fn format_price(&self, price: Price) -> String {
        price.format(self.price_decimals)
    }

    /// Format a quantity for display with this instrument's precision.
    pub fn format_qty(&self, qty: Qty) -> String {
        qty.format(self.qty_decimals)
    }

    /// Whether a price sits exactly on a tick boundary.
    #[inline]
    pub fn is_on_tick(&self, price: Price) -> bool {
        price.minor().rem_euclid(self.tick_size.minor()) == 0
    }
}

/// Infer a sensible display precision from an increment's minor units.
///
/// A tick of `0.01` (1_000_000 minor units) should display 2 decimals.
fn decimals_for(step_minor: i64) -> u32 {
    if step_minor <= 0 {
        return crate::fixed::DECIMALS;
    }
    let mut remaining = step_minor;
    let mut decimals = crate::fixed::DECIMALS;
    while decimals > 0 && remaining % 10 == 0 {
        remaining /= 10;
        decimals -= 1;
    }
    decimals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btc() -> Instrument {
        Instrument::spot(
            "BTCUSDT",
            Venue::Binance,
            Price::parse("0.01").unwrap(),
            Qty::parse("0.00001").unwrap(),
        )
    }

    #[test]
    fn infers_display_precision_from_increments() {
        let inst = btc();
        assert_eq!(inst.price_decimals, 2);
        assert_eq!(inst.qty_decimals, 5);

        let es = Instrument::spot(
            "ES",
            Venue::Sim,
            Price::parse("0.25").unwrap(),
            Qty::parse("1").unwrap(),
        );
        assert_eq!(es.price_decimals, 2);
        assert_eq!(es.qty_decimals, 0);
    }

    #[test]
    fn maps_prices_to_ladder_rows() {
        let inst = btc();
        let p = Price::parse("95000.00").unwrap();
        let idx = inst.tick_index(p);
        assert_eq!(inst.price_at(idx), p);
        // One tick up is exactly one row up.
        assert_eq!(inst.price_at(idx + 1), Price::parse("95000.01").unwrap());
    }

    #[test]
    fn detects_off_tick_prices() {
        let inst = btc();
        assert!(inst.is_on_tick(Price::parse("95000.01").unwrap()));
        assert!(!inst.is_on_tick(Price::parse("95000.015").unwrap()));
    }

    #[test]
    fn formats_with_instrument_precision() {
        let inst = btc();
        assert_eq!(inst.format_price(Price::parse("95000.5").unwrap()), "95000.50");
        assert_eq!(inst.format_qty(Qty::parse("0.5").unwrap()), "0.50000");
    }

    #[test]
    fn key_is_venue_qualified() {
        assert_eq!(btc().key(), "binance:BTCUSDT");
    }

    #[test]
    fn settlement_currency_follows_contract_kind() {
        assert!(InstrumentKind::Spot.is_quote_settled());
        assert!(InstrumentKind::LinearPerp.is_quote_settled());
        assert!(!InstrumentKind::InversePerp.is_quote_settled());
    }
}
