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
  trading/   paper matching, positions and PnL
  app/       the session state machine and UI data types
ui/          TypeScript front end: footprint chart, DOM ladder, tape
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

Requires Rust 1.82 or newer, and Node 20+ for the UI.

```bash
cargo test --workspace                                    # 227 tests
cargo clippy --workspace --all-targets -- -D warnings

cd ui
npm install
npm run typecheck        # strict TypeScript, no implicit any
npm run build
npm run test:render      # needs: npx playwright install chromium
```

The UI runs in a plain browser as well as in the desktop shell. Outside Tauri
it falls back to a seeded mock transport that emits the same DTO shapes the
Rust session produces, so the renderers can be developed and screenshot-tested
without a desktop build:

```bash
cd ui && npm run dev     # http://localhost:5173
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
| `atas-trading` — paper matching, positions, PnL | **Done**, 27 tests |
| `atas-app` — session state machine, UI DTOs | **Done**, 20 tests |
| Tauri shell — window, commands, event bridge | Not started |
| `ui/` — footprint chart, DOM ladder, tape | **Done**, render-tested |
| UI — dockable multi-pane workspaces | Not started |

### Why the session layer has no Tauri in it

`atas-app` holds everything the desktop app does to market data, in plain
Rust with no window toolkit anywhere near it. The Tauri shell is a transport:
it hands market events to a `Session` and forwards the `AppEvent`s that come
back. Logic that can only be exercised by launching a window is logic that
does not get tested — this crate's 20 tests drive the entire platform end to
end, from a synthetic feed through aggregation, indicators and paper fills to
a serialised frame, with no display involved.

Prices cross into the UI as fixed-point *minor units*, never decimals.
JavaScript numbers are IEEE doubles, exact for integers below 2^53 (~9.0e15);
minor units at 8 decimal places put a six-figure price near 9.5e12, four
orders of magnitude inside that bound. Sending `95000.01` as a JSON decimal
would hand the UI a float to compare against — reintroducing at the boundary
the exact problem fixed point exists to avoid.

### Why paper fills are pessimistic

A paper engine is only useful if it is pessimistic where a real venue would
be. Market orders walk the book level by level and report the volume-weighted
result, so size too large for the depth shows up as the slippage it is —
filling everything at the touch would hide most of the cost of trading size.
Resting limit orders fill only once the market trades *through* them, never
merely *at* them: without a real queue-position model, the honest assumption
is that the trader was behind everyone already queued at that price. That
understates fills slightly, which is the correct direction to be wrong in.

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
