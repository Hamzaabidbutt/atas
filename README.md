# ATAS

An order-flow analysis and trading platform: cluster (footprint) charts,
market profile, a depth-of-market ladder and paper trading, built as a native
desktop application.

**Status: early. The core engine is built and tested; the application shell and
UI are not yet.** See [Roadmap](#roadmap) for exactly what exists today.

## Architecture

Rust core, TypeScript UI, packaged with Tauri. The split is deliberate: tick
ingestion, storage and cluster aggregation are latency- and allocation-
sensitive and live in Rust; the chart and workspace layer is a UI problem and
lives in TypeScript against a canvas.

```
crates/
  core/      domain vocabulary: fixed-point Price/Qty, Instrument, Trade, OrderBook
  engine/    bar construction and cluster/footprint computation
  store/     segmented append-only tick history
  feed/      venue wire formats, replay and synthetic feeds
  indicators/ order-flow and classical studies
ui/          TypeScript front end (not started)
web/         the marketing site, static HTML/CSS/JS
```

### Why fixed point

Prices and quantities are `i64` minor units at 8 decimal places, never floats.
A cluster ladder keys cells by exact price; under floating point
`0.1 + 0.2 != 0.3`, so a single price level silently becomes two and the
footprint is wrong in a way that is very hard to see. Every price comparison,
tick-index mapping and ladder row in the system depends on exact equality.

### Why diagonal imbalance

Footprint imbalance compares the ask volume at a price against the **bid volume
one tick below**, not against the bid on the same row. Those are the two sides
of the same auction; buyers lifting the offer at 100.25 were being filled by
sellers resting at 100.00. Comparing bid and ask on one row measures two
populations that never traded against each other. `ClusterLadder::imbalances`
implements the diagonal form, and `stacked_imbalances` finds the consecutive
runs that are the actual tradable signal.

## Building

Requires Rust 1.82 or newer.

```bash
cargo test --workspace        # unit and integration tests
cargo clippy --workspace --all-targets -- -D warnings
```

## Roadmap

| Component | Status |
| --- | --- |
| `atas-core` — prices, instruments, trades, L2 book | **Done**, 40 tests |
| `atas-engine` — bars and cluster ladders | **Done**, 41 tests |
| `atas-store` — segmented on-disk tick history | **Done**, 25 tests |
| `atas-feed` — wire formats, replay, synthetic | **Done**, 46 tests |
| `atas-indicators` — CVD, VWAP, profiles, scanners | **Done**, 28 tests |
| `atas-feed` — live WebSocket/REST transport | Not started |
| Paper trading — matching, positions, PnL | Not started |
| Tauri shell — state, commands, event bus | Not started |
| UI — footprint chart, DOM, tape, workspaces | Not started |

### Why the aggressor field is isolated per venue

Binance reports `m` — *"was the buyer the maker?"* — so `m == true` means the
**seller** crossed the spread. Bybit reports the taker side directly. Reading
either one wrong inverts delta, every imbalance, and the sign of CVD across
the whole platform while still drawing plausible-looking charts. Each venue's
conversion is a single named function with its own direct test, and the two
adapters deliberately do not share one.

Known gaps in what is built:

- Renko bars are not implemented. Splitting a multi-brick move's cluster ladder
  across bricks correctly needs ladder range-extraction that does not exist
  yet, and shipping an approximation would put wrong volume at wrong prices.
- Volume and delta bars do not split the trade that crosses their threshold, so
  a bar may overshoot. This matches how most platforms behave and keeps a trade
  atomic in one bar.
- The Binance and Bybit adapters translate wire formats but do not yet open
  sockets. Transport — connect, resubscribe, sequence-gap recovery, REST
  backfill — is the remaining part of that crate. It cannot be verified in the
  environment this was built in, which has no exchange network access, so it is
  being written after everything that can be tested offline.
- Bybit trade ids are UUIDs and do not fit the store's `u64` id field, so they
  are stored as zero rather than truncated into something that could collide.
  Deduplication for that venue keys on `(ts, price, qty)` instead.

## The marketing site

`web/` holds a static replica of an order-flow platform's marketing site,
built earlier as a design exercise. Serve it with any static server:

```bash
cd web && python3 -m http.server 8000
```

It is an independent, non-commercial replica, not affiliated with ATAS, and
every price, statistic and quote on it is an illustrative placeholder.
