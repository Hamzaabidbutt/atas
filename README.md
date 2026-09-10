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
src-tauri/   desktop shell (its own workspace — see below)
web/         the marketing site, static HTML/CSS/JS
```

### Why fixed point

Prices and quantities are `i64` minor units at 8 decimal places, never floats.
A cluster ladder keys cells by exact price; under floating point
`0.1 + 0.2 != 0.3`, so a single price level silently becomes two and the
footprint is wrong in a way that is very hard to see. Every price comparison,
tick-index mapping and ladder row in the system depends on exact equality.

### Row height is not tick size

A footprint row is *not* one instrument tick. BTCUSDT ticks at 0.01, so a bar
spanning a few dollars would have hundreds of rows — unreadable, and no more
informative than a heatmap. `LadderSpec` keeps the two apart: `tick_size` is
the instrument's increment, which order prices quantise to, and
`ticks_per_row` sets how many of those a footprint row spans.

Conflating them means choosing between an illegible ladder and an instrument
whose order prices are wrong. In the desktop shell BTCUSDT keeps its real 0.01
increment — the DOM ladder shows one-cent levels — while the chart aggregates
25 ticks into each 0.25 row.

Two consequences worth knowing:

- **Range bars measure instrument ticks, not rows.** A bar's height is a
  property of the market; if range followed the row size, changing a display
  setting would silently change which bars exist.
- **Changing the row height clears chart history.** Existing bars were
  bucketed at the old height, and re-bucketing them would need the ticks they
  were built from, which bars no longer carry. A replay from stored ticks can
  rebuild them; the session cannot.

### Why diagonal imbalance

Footprint imbalance compares the ask volume at a price against the **bid volume
one tick below**, not against the bid on the same row. Those are the two sides
of the same auction; buyers lifting the offer at 100.25 were being filled by
sellers resting at 100.00. Comparing bid and ask on one row measures two
populations that never traded against each other. `ClusterLadder::imbalances`
implements the diagonal form, and `stacked_imbalances` finds the consecutive
runs that are the actual tradable signal.

## Running it

### The desktop app

Requires **Rust 1.82+** and **Node 20+**. On Debian or Ubuntu you also need the
Tauri system libraries:

```bash
sudo apt-get install libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev \
                     libjavascriptcoregtk-4.1-dev librsvg2-dev patchelf
```

macOS needs Xcode command line tools.

**Windows** needs Git, Rust, Node, the MSVC build tools (Rust links with MSVC
on Windows) and the WebView2 runtime, which is preinstalled on Windows 11.
`scripts/setup.ps1` installs all of them through winget.

Then, from the repository root:

```bash
./scripts/setup.sh            # Linux and macOS
```
```powershell
.\scripts\setup.ps1          # Windows PowerShell
```

That checks and installs the toolchains and system libraries, pulls
dependencies, runs the test suite, and offers to launch the app. It is safe to
re-run — every step checks whether it is already satisfied. Use
`./scripts/setup.sh --check` to see what is missing without changing anything,
or `--run` to launch without being asked.

To do it by hand instead:

```bash
npm install          # the Tauri CLI
npm run setup        # the UI's own dependencies
npm run dev          # launches the app with hot reload
```

For a standalone binary:

```bash
npm run build                              # or: npm run build -- --no-bundle
./src-tauri/target/release/atas-desktop
```

`npm run build` also produces installers (`.deb`, `.AppImage`, `.dmg`, `.msi`)
under `src-tauri/target/release/bundle/`. Pass `--no-bundle` to skip those and
build only the executable, which is much faster.

> **Build through the Tauri CLI, not `cargo build`.** A plain
> `cargo build --release` inside `src-tauri` produces a binary that still
> points at the dev server on `localhost:5173` and shows "Could not connect to
> localhost" when run on its own — cargo's profile is not what decides whether
> the frontend is embedded; the CLI is.

The app starts on a built-in synthetic feed, so it works with no network and no
exchange account. Everything on screen is simulated.

### The library crates

```bash
cargo test --workspace                                    # 271 tests
cargo clippy --workspace --all-targets -- -D warnings

cd ui
npm run typecheck        # strict TypeScript
npm run test:render      # needs: npx playwright install chromium
```

### Why `src-tauri` is its own workspace

Tauri needs system GUI libraries that a headless CI runner or a server
checkout will not have. As a member of the root workspace it would break
`cargo test --workspace` everywhere those are missing, so it stands alone and
is built through the Tauri CLI instead.

### Why the desktop shell is nearly empty

Everything the app does to market data lives in `atas-app` — the session state
machine and the pump loop, both plain Rust with tests. What is left in
`src-tauri` is only what genuinely needs a window: holding the session behind
a lock, running the pump on a background thread, forwarding events to the
webview, and translating commands. The pump holds the session lock only while
draining a bounded batch, so UI commands are never queued behind a burst of
market data.

Order quantities cross that boundary as **decimal strings, not numbers**.
The Rust side is fixed point precisely so an order size never passes through
a float, and the IPC boundary is the easiest place to undo that by accident.

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
- Bar rules and feed rates have to be chosen together. A one-minute bar
  against a feed printing every 25ms is 2,400 trades in one bar, spanning far
  too many price levels to render legibly. There is no guard against
  configuring that combination.
- **The live socket transport has never been run against a venue.** It was
  written in an environment with no exchange network access, so it is
  compile-verified and built on tested components — the depth handshake
  (`sync.rs`) and reconnect pacing (`backoff.rs`) are exhaustively unit-tested
  pure state machines — but the first real socket it opens will be on your
  machine. It lives behind `--features live` so the tested parts of the crate
  build and test without an async runtime or a TLS stack. Historical REST
  backfill is not implemented at all.
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
