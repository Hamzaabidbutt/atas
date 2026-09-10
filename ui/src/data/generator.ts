import type {
  AppEvent,
  BarDto,
  BookDto,
  ClusterDto,
  ImbalanceDto,
  SnapshotDto,
  TapeRowDto,
} from "../types";
import type { Transport } from "../transport";

/**
 * A browser-side stand-in for the Rust session.
 *
 * It emits exactly the DTO shapes `crates/app` produces, so the panes cannot
 * tell the difference. Used for developing and screenshot-testing the
 * renderers without a desktop build, and seeded so a rendering bug reproduces
 * instead of scrolling away.
 */

const SCALE = 100_000_000;
const TICK = 25_000_000; // 0.25

/** xorshift64, so the same seed gives the same chart in every browser. */
function rng(seed: number): () => number {
  let a = seed >>> 0 || 0x9e3779b9;
  return () => {
    a ^= a << 13;
    a >>>= 0;
    a ^= a >>> 17;
    a ^= a << 5;
    a >>>= 0;
    return a / 0x1_0000_0000;
  };
}

export interface GeneratorOptions {
  seed?: number;
  bars?: number;
  anchor?: number;
  /** Milliseconds between generated trades. Zero disables live updates. */
  intervalMs?: number;
}

export class MockTransport implements Transport {
  readonly label = "mock";

  private readonly random: () => number;
  private readonly anchorIndex: number;
  private priceIndex: number;
  private ts = Date.now() * 1_000_000;
  private bars: BarDto[] = [];
  private tape: TapeRowDto[] = [];
  private cvd = 0;
  private handlers = new Set<(e: AppEvent) => void>();
  private timer: number | undefined;
  private tradesInBar = 0;
  private readonly tradesPerBar = 40;
  private readonly intervalMs: number;
  private vwapWeighted = 0;
  private vwapVolume = 0;

  constructor(options: GeneratorOptions = {}) {
    this.random = rng(options.seed ?? 20_260_910);
    this.anchorIndex = Math.round((options.anchor ?? 95_000) * SCALE / TICK);
    this.priceIndex = this.anchorIndex;
    this.intervalMs = options.intervalMs ?? 120;

    for (let i = 0; i < (options.bars ?? 40); i++) {
      this.bars.push(this.makeBar());
    }
  }

  /**
   * Mean-reverting walk.
   *
   * The step is deliberately small: at +/-2 ticks per trade over 40 trades a
   * bar wanders far enough that consecutive bars stop overlapping in price,
   * and the chart becomes a row of disconnected islands rather than a
   * footprint. One tick per trade with a firm pull keeps bars adjacent.
   */
  private step(): void {
    // Reversion is applied to the *probability* of each direction rather than
    // as a separate correction step. A threshold-triggered pull leaves the
    // walk unbiased inside the threshold, so it trends freely and the chart
    // becomes a staircase instead of a market.
    const distance = this.anchorIndex - this.priceIndex;
    const bias = Math.max(-0.28, Math.min(0.28, distance / 45));
    this.priceIndex += this.random() < 0.5 + bias ? 1 : -1;
  }

  private makeBar(): BarDto {
    const openIndex = this.priceIndex;
    const levels = new Map<number, { bid: number; ask: number; trades: number }>();
    let high = openIndex;
    let low = openIndex;
    let volume = 0;
    let delta = 0;
    const openTs = this.ts;

    for (let i = 0; i < this.tradesPerBar; i++) {
      this.step();
      high = Math.max(high, this.priceIndex);
      low = Math.min(low, this.priceIndex);

      const u = this.random();
      const size = Math.round((1 + u * u * u * 300) * SCALE);
      const buy = this.random() > 0.48;

      const cell = levels.get(this.priceIndex) ?? { bid: 0, ask: 0, trades: 0 };
      if (buy) cell.ask += size;
      else cell.bid += size;
      cell.trades += 1;
      levels.set(this.priceIndex, cell);

      volume += size;
      delta += buy ? size : -size;
      this.ts += 250_000_000;
    }

    return this.finishBar(openIndex, high, low, volume, delta, levels, openTs, true);
  }

  private finishBar(
    openIndex: number,
    high: number,
    low: number,
    volume: number,
    delta: number,
    levels: Map<number, { bid: number; ask: number; trades: number }>,
    openTs: number,
    closed: boolean,
  ): BarDto {
    // Dense ladder between low and high, matching ClusterLadder: untouched
    // levels are present and empty, because gaps carry information.
    const clusters: ClusterDto[] = [];
    for (let index = low; index <= high; index++) {
      const cell = levels.get(index) ?? { bid: 0, ask: 0, trades: 0 };
      clusters.push({
        price: index * TICK,
        bid: cell.bid,
        ask: cell.ask,
        trades: cell.trades,
      });
    }

    let poc: number | null = null;
    let best = -1;
    for (const cell of clusters) {
      const total = cell.bid + cell.ask;
      if (total > best) {
        best = total;
        poc = cell.price;
      }
    }

    // Diagonal imbalance, matching the Rust engine: ask at a level against bid
    // one tick below, never bid against ask on the same row.
    const imbalances: ImbalanceDto[] = [];
    for (let i = 0; i < clusters.length; i++) {
      const cell = clusters[i]!;
      const below = i > 0 ? clusters[i - 1]! : undefined;
      const above = i + 1 < clusters.length ? clusters[i + 1]! : undefined;
      if (below && cell.ask > 0 && cell.ask >= below.bid * 3) {
        imbalances.push({
          price: cell.price,
          side: "buy",
          ratio: below.bid > 0 ? cell.ask / below.bid : null,
        });
      }
      if (above && cell.bid > 0 && cell.bid >= above.ask * 3) {
        imbalances.push({
          price: cell.price,
          side: "sell",
          ratio: above.ask > 0 ? cell.bid / above.ask : null,
        });
      }
    }

    this.cvd += delta;
    this.vwapWeighted += (this.priceIndex * TICK / SCALE) * volume;
    this.vwapVolume += volume;

    return {
      open_ts: openTs,
      close_ts: this.ts,
      open: openIndex * TICK,
      high: high * TICK,
      low: low * TICK,
      close: this.priceIndex * TICK,
      volume,
      delta,
      trades: this.tradesPerBar,
      closed,
      poc,
      clusters,
      imbalances,
    };
  }

  private makeBook(): BookDto {
    const bids = [];
    const asks = [];
    for (let i = 0; i < 12; i++) {
      bids.push({
        price: (this.priceIndex - 1 - i) * TICK,
        qty: Math.round((1 + this.random() * 400) * SCALE),
      });
      asks.push({
        price: (this.priceIndex + 1 + i) * TICK,
        qty: Math.round((1 + this.random() * 400) * SCALE),
      });
    }
    return {
      bids,
      asks,
      spread: 2 * TICK,
      crossed: false,
    };
  }

  async snapshot(): Promise<SnapshotDto> {
    // Copy the arrays. Handing out the live ones lets the generator's own
    // push and the consumer's event handler both append the same bar, which
    // renders it twice. Tauri serialises across the IPC boundary, so the real
    // transport cannot exhibit this — and the mock must not either, or it
    // stops being a faithful stand-in.
    const vwap =
      this.vwapVolume > 0
        ? Math.round((this.vwapWeighted / this.vwapVolume) * SCALE)
        : null;
    return {
      instrument: "sim:BTCUSDT",
      scale: SCALE,
      tick_size: TICK,
      price_decimals: 2,
      bars: this.bars.map((bar) => ({ ...bar })),
      book: this.makeBook(),
      tape: [...this.tape],
      indicators: {
        cvd: this.cvd,
        divergence: null,
        vwap,
        vwap_deviation: vwap === null ? null : Math.round(12 * SCALE),
        session_poc: this.bars.at(-1)?.poc ?? null,
        value_area_high: null,
        value_area_low: null,
        speed_of_tape: 8.4,
      },
      position: {
        qty: 0,
        avg_price: 0,
        realised: 0,
        unrealised: 0,
        commission: 0,
        side: null,
      },
      orders: [],
      last_price: this.priceIndex * TICK,
      last_ts: this.ts,
    };
  }

  subscribe(handler: (event: AppEvent) => void): () => void {
    this.handlers.add(handler);
    if (this.intervalMs > 0 && this.timer === undefined) {
      this.timer = setInterval(() => this.tick(), this.intervalMs) as unknown as number;
    }
    return () => {
      this.handlers.delete(handler);
      if (this.handlers.size === 0 && this.timer !== undefined) {
        clearInterval(this.timer);
        this.timer = undefined;
      }
    };
  }

  private emit(event: AppEvent): void {
    for (const handler of this.handlers) handler(event);
  }

  /** Produce one trade and the events it causes. */
  private tick(): void {
    this.step();
    const u = this.random();
    const size = Math.round((1 + u * u * u * 300) * SCALE);
    const buy = this.random() > 0.48;
    this.ts += this.intervalMs * 1_000_000;

    const row: TapeRowDto = {
      ts: this.ts,
      price: this.priceIndex * TICK,
      qty: size,
      side: buy ? "buy" : "sell",
    };
    this.tape.push(row);
    if (this.tape.length > 200) this.tape.shift();
    this.emit({ type: "tape_row", 0: row });

    if (size > 150 * SCALE) {
      this.emit({
        type: "big_trade",
        ts: row.ts,
        price: row.price,
        qty: row.qty,
        side: row.side,
      });
    }

    this.tradesInBar += 1;
    if (this.tradesInBar >= this.tradesPerBar) {
      this.tradesInBar = 0;
      const bar = this.makeBar();
      this.bars.push(bar);
      if (this.bars.length > 400) this.bars.shift();
      this.emit({ type: "bar_closed", 0: bar });
    }

    this.emit({ type: "book_updated", 0: this.makeBook() });
  }

  async command(name: string): Promise<unknown> {
    return { ok: true, command: name };
  }
}
