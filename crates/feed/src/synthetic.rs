//! A deterministic synthetic feed.
//!
//! Order-flow features are hard to develop against a live socket: the data is
//! different every run, and this build environment has no exchange access at
//! all. This generator produces a reproducible stream — same seed, same ticks,
//! every time — so a rendering bug or an off-by-one in an indicator is
//! debuggable rather than a race with the market.

use atas_core::{Instrument, MarketEvent, Price, Qty, Side, Trade, Ts};

use crate::error::FeedError;
use crate::Feed;

/// Generates a reproducible trade and book stream.
#[derive(Debug, Clone)]
pub struct SyntheticFeed {
    tick_size: Price,
    state: u64,
    price_index: i64,
    anchor_index: i64,
    ts: Ts,
    step_nanos: i64,
    emitted: u64,
    limit: Option<u64>,
    book_every: u64,
    depth: usize,
}

impl SyntheticFeed {
    /// A feed for an instrument, seeded for reproducibility.
    pub fn new(instrument: &Instrument, seed: u64, start_price: Price) -> Self {
        let anchor = start_price.tick_index(instrument.tick_size);
        Self {
            tick_size: instrument.tick_size,
            // Zero is a fixed point of the xorshift step, so never seed with it.
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
            price_index: anchor,
            anchor_index: anchor,
            ts: Ts::from_millis(1_700_000_000_000),
            step_nanos: atas_core::time::NANOS_PER_MILLI * 25,
            emitted: 0,
            limit: None,
            book_every: 20,
            depth: 10,
        }
    }

    /// Stop after `count` events.
    pub fn with_limit(mut self, count: u64) -> Self {
        self.limit = Some(count);
        self
    }

    /// Set the nanoseconds between generated events.
    pub fn with_step_nanos(mut self, nanos: i64) -> Self {
        assert!(nanos > 0, "step must be positive");
        self.step_nanos = nanos;
        self
    }

    /// Emit a book snapshot every `n` events. Zero disables book output.
    pub fn with_book_every(mut self, n: u64) -> Self {
        self.book_every = n;
        self
    }

    /// Set the first timestamp.
    pub fn starting_at(mut self, ts: Ts) -> Self {
        self.ts = ts;
        self
    }

    /// How many events have been produced.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// xorshift64: small, fast, and identical on every platform, which a
    /// hashing PRNG from the standard library is not guaranteed to be.
    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn price_at(&self, index: i64) -> Price {
        Price::from_tick_index(index, self.tick_size)
    }

    /// Mean-reverting random walk, so a long run cannot wander to absurd
    /// prices or to zero.
    fn step_price(&mut self) {
        let r = self.next_u64();
        let drift = (r % 5) as i64 - 2;
        let pull = (self.anchor_index - self.price_index) / 64;
        self.price_index += drift + pull;
    }

    fn make_trade(&mut self) -> Trade {
        self.step_price();
        let r = self.next_u64();
        // Cube the uniform draw so most prints are small and the occasional
        // one is large, which is what a real tape looks like.
        let unit = (r >> 20) as f64 / (u64::MAX >> 20) as f64;
        let size = 1 + (unit * unit * unit * 400.0) as i64;

        let trade = Trade {
            ts: self.ts,
            price: self.price_at(self.price_index),
            qty: Qty::from_units(size),
            aggressor: if (r >> 8) % 2 == 0 {
                Side::Buy
            } else {
                Side::Sell
            },
            id: self.emitted,
        };
        self.ts = self.ts + self.step_nanos;
        trade
    }

    fn make_book(&mut self) -> MarketEvent {
        let mut bids = Vec::with_capacity(self.depth);
        let mut asks = Vec::with_capacity(self.depth);
        for level in 0..self.depth as i64 {
            let size = 1 + (self.next_u64() % 500) as i64;
            bids.push((
                self.price_at(self.price_index - 1 - level),
                Qty::from_units(size),
            ));
            let size = 1 + (self.next_u64() % 500) as i64;
            asks.push((
                self.price_at(self.price_index + 1 + level),
                Qty::from_units(size),
            ));
        }
        MarketEvent::BookSnapshot {
            ts: self.ts,
            bids,
            asks,
            sequence: self.emitted + 1,
        }
    }
}

impl Feed for SyntheticFeed {
    fn next_event(&mut self) -> Result<Option<MarketEvent>, FeedError> {
        if self.limit.is_some_and(|n| self.emitted >= n) {
            return Ok(None);
        }

        let event = if self.book_every > 0 && self.emitted % self.book_every == self.book_every - 1 {
            self.make_book()
        } else {
            MarketEvent::Trade(self.make_trade())
        };
        self.emitted += 1;
        Ok(Some(event))
    }
}
