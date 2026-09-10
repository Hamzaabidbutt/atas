import type { BarDto, SnapshotDto } from "../types";
import { formatMinor, formatVolume } from "../types";
import { mono, monoSmall, prepare, theme } from "../theme";

/**
 * The footprint (cluster) chart.
 *
 * Each bar is drawn as a column of per-price cells showing bid volume against
 * ask volume, with diagonal imbalances highlighted and the bar's point of
 * control outlined. The candle body is drawn behind the ladder for context,
 * but the ladder is the content — the whole reason to look at this instead of
 * a candlestick chart.
 */

export interface FootprintOptions {
  /** Bar column width in CSS pixels. */
  columnWidth: number;
  /** Maximum cell height; the chart shrinks cells to fit the price range. */
  maxCellHeight: number;
  /** Hide per-cell numbers below this height and draw a heatmap instead. */
  minTextHeight: number;
}

const DEFAULTS: FootprintOptions = {
  columnWidth: 78,
  maxCellHeight: 16,
  minTextHeight: 11,
};

const PADDING = { top: 26, right: 74, bottom: 26, left: 8 };

export class FootprintChart {
  private options: FootprintOptions;
  /** How many columns the viewport is scrolled back from the newest bar. */
  private scrollBack = 0;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    options: Partial<FootprintOptions> = {},
  ) {
    this.options = { ...DEFAULTS, ...options };
  }

  /** Scroll the viewport by whole columns. Clamped by {@link render}. */
  scrollBy(columns: number): void {
    this.scrollBack = Math.max(0, this.scrollBack + columns);
  }

  /** Jump back to the newest bar. */
  scrollToLive(): void {
    this.scrollBack = 0;
  }

  /** Whether the viewport is pinned to the newest bar. */
  get isLive(): boolean {
    return this.scrollBack === 0;
  }

  render(snapshot: SnapshotDto): void {
    const surface = prepare(this.canvas);
    if (!surface) return;
    const { ctx, width, height } = surface;

    ctx.fillStyle = theme.bg;
    ctx.fillRect(0, 0, width, height);

    const plotWidth = width - PADDING.left - PADDING.right;
    const plotHeight = height - PADDING.top - PADDING.bottom;
    if (plotWidth <= 0 || plotHeight <= 0 || snapshot.bars.length === 0) {
      this.drawEmpty(ctx, width, height);
      return;
    }

    const columns = Math.max(1, Math.floor(plotWidth / this.options.columnWidth));
    // Clamp the scroll so it can never run past the start of history.
    const maxScroll = Math.max(0, snapshot.bars.length - columns);
    this.scrollBack = Math.min(this.scrollBack, maxScroll);

    const end = snapshot.bars.length - this.scrollBack;
    const visible = snapshot.bars.slice(Math.max(0, end - columns), end);
    if (visible.length === 0) return;

    // Price scale spans the visible bars, with a little headroom.
    let hi = -Infinity;
    let lo = Infinity;
    for (const bar of visible) {
      hi = Math.max(hi, bar.high);
      lo = Math.min(lo, bar.low);
    }
    const pad = Math.max((hi - lo) * 0.06, snapshot.row_size * 2);
    hi += pad;
    lo -= pad;
    const span = Math.max(hi - lo, snapshot.row_size);

    const y = (price: number) =>
      PADDING.top + ((hi - price) / span) * plotHeight;

    // Cell height follows the *row* size, not the instrument tick. Using the
    // tick would draw one-cent cells for an instrument whose footprint
    // aggregates 25 ticks to a row, leaving gaps between every drawn cell.
    const rowPixels = (snapshot.row_size / span) * plotHeight;
    const cellHeight = Math.max(2, Math.min(this.options.maxCellHeight, rowPixels));
    const showText = cellHeight >= this.options.minTextHeight;

    this.drawGrid(ctx, snapshot, width, plotHeight, lo, hi, y);

    ctx.save();
    ctx.beginPath();
    ctx.rect(PADDING.left, 0, plotWidth, height);
    ctx.clip();

    visible.forEach((bar, i) => {
      this.drawBar(ctx, snapshot, bar, PADDING.left + i * this.options.columnWidth, y, cellHeight, showText, height);
    });

    ctx.restore();
    this.drawLastPrice(ctx, snapshot, width, y);
    this.drawHeader(ctx, snapshot, visible, width);
  }

  private drawEmpty(
    ctx: CanvasRenderingContext2D,
    width: number,
    height: number,
  ): void {
    ctx.fillStyle = theme.muted;
    ctx.font = mono;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText("waiting for data", width / 2, height / 2);
  }

  private drawGrid(
    ctx: CanvasRenderingContext2D,
    snapshot: SnapshotDto,
    width: number,
    plotHeight: number,
    lo: number,
    hi: number,
    y: (p: number) => number,
  ): void {
    const steps = Math.max(2, Math.min(10, Math.floor(plotHeight / 46)));
    ctx.font = monoSmall;
    ctx.textBaseline = "middle";

    for (let i = 0; i <= steps; i++) {
      const price = lo + ((hi - lo) / steps) * i;
      const gy = Math.round(y(price)) + 0.5;
      ctx.strokeStyle = theme.grid;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(PADDING.left, gy);
      ctx.lineTo(width - PADDING.right, gy);
      ctx.stroke();

      ctx.fillStyle = theme.muted;
      ctx.textAlign = "left";
      ctx.fillText(
        formatMinor(price, snapshot.scale, snapshot.price_decimals),
        width - PADDING.right + 7,
        gy,
      );
    }

    // VWAP is a level traders act on, so it gets its own line.
    if (snapshot.indicators.vwap !== null) {
      const vy = Math.round(y(snapshot.indicators.vwap)) + 0.5;
      ctx.strokeStyle = theme.vwap;
      ctx.globalAlpha = 0.75;
      ctx.setLineDash([5, 4]);
      ctx.beginPath();
      ctx.moveTo(PADDING.left, vy);
      ctx.lineTo(width - PADDING.right, vy);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;

      ctx.fillStyle = theme.vwap;
      ctx.font = monoSmall;
      ctx.textAlign = "left";
      ctx.fillText("VWAP", PADDING.left + 4, vy - 7);
    }
  }

  private drawBar(
    ctx: CanvasRenderingContext2D,
    snapshot: SnapshotDto,
    bar: BarDto,
    x: number,
    y: (p: number) => number,
    cellHeight: number,
    showText: boolean,
    height: number,
  ): void {
    const up = bar.close >= bar.open;
    const colour = up ? theme.buy : theme.sell;
    const cellWidth = this.options.columnWidth - 14;
    const cellX = x + 7;

    // Wick and body sit behind the ladder for candle context.
    ctx.strokeStyle = colour;
    ctx.globalAlpha = 0.5;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(Math.round(x + 3) + 0.5, y(bar.high));
    ctx.lineTo(Math.round(x + 3) + 0.5, y(bar.low));
    ctx.stroke();
    ctx.globalAlpha = 1;

    const bodyTop = y(Math.max(bar.open, bar.close));
    const bodyBottom = y(Math.min(bar.open, bar.close));
    ctx.fillStyle = up ? theme.buyFill : theme.sellFill;
    ctx.fillRect(cellX, bodyTop, cellWidth, Math.max(1.5, bodyBottom - bodyTop));

    let peak = 1;
    for (const cell of bar.clusters) peak = Math.max(peak, cell.bid + cell.ask);

    const imbalanced = new Map(bar.imbalances.map((i) => [i.price, i.side]));

    for (const cell of bar.clusters) {
      const cy = y(cell.price) - cellHeight / 2;
      if (cy + cellHeight < 0 || cy > height) continue;

      const side = imbalanced.get(cell.price);
      if (side === "buy") ctx.fillStyle = theme.buyHot;
      else if (side === "sell") ctx.fillStyle = theme.sellHot;
      else {
        // Heatmap shading by relative volume, so structure is legible even
        // when the cells are too small for numbers.
        const strength = (cell.bid + cell.ask) / peak;
        ctx.fillStyle = `rgba(255,255,255,${(0.02 + strength * 0.07).toFixed(3)})`;
      }
      ctx.fillRect(cellX, cy, cellWidth, Math.max(1, cellHeight - 1.5));

      if (cell.price === bar.poc) {
        ctx.strokeStyle = theme.poc;
        ctx.globalAlpha = 0.8;
        ctx.lineWidth = 1;
        ctx.strokeRect(cellX + 0.5, cy + 0.5, cellWidth - 1, Math.max(1, cellHeight - 2.5));
        ctx.globalAlpha = 1;
      }

      if (showText) {
        const mid = cy + cellHeight / 2;
        ctx.font = monoSmall;
        ctx.textBaseline = "middle";
        ctx.fillStyle = theme.sell;
        ctx.textAlign = "right";
        ctx.fillText(formatVolume(cell.bid, snapshot.scale), cellX + cellWidth / 2 - 3, mid);
        ctx.fillStyle = theme.buy;
        ctx.textAlign = "left";
        ctx.fillText(formatVolume(cell.ask, snapshot.scale), cellX + cellWidth / 2 + 3, mid);
      }
    }

    // Delta footer: the single number that summarises the bar's aggression.
    ctx.font = monoSmall;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillStyle = bar.delta >= 0 ? theme.buy : theme.sell;
    const sign = bar.delta >= 0 ? "+" : "";
    ctx.fillText(
      `${sign}${formatVolume(bar.delta, snapshot.scale)}`,
      x + this.options.columnWidth / 2,
      height - PADDING.bottom + 11,
    );
  }

  private drawLastPrice(
    ctx: CanvasRenderingContext2D,
    snapshot: SnapshotDto,
    width: number,
    y: (p: number) => number,
  ): void {
    if (snapshot.last_price === null) return;
    const last = snapshot.bars.at(-1);
    const up = last ? last.close >= last.open : true;
    const py = Math.round(y(snapshot.last_price)) + 0.5;

    ctx.strokeStyle = up ? theme.buy : theme.sell;
    ctx.globalAlpha = 0.7;
    ctx.setLineDash([3, 4]);
    ctx.beginPath();
    ctx.moveTo(PADDING.left, py);
    ctx.lineTo(width - PADDING.right, py);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    const label = formatMinor(snapshot.last_price, snapshot.scale, snapshot.price_decimals);
    ctx.font = monoSmall;
    const boxWidth = ctx.measureText(label).width + 12;
    ctx.fillStyle = up ? theme.buy : theme.sell;
    ctx.fillRect(width - PADDING.right + 2, py - 8, boxWidth, 16);
    ctx.fillStyle = "#04150f";
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText(label, width - PADDING.right + 8, py);
  }

  private drawHeader(
    ctx: CanvasRenderingContext2D,
    snapshot: SnapshotDto,
    visible: BarDto[],
    width: number,
  ): void {
    ctx.font = monoSmall;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.fillStyle = theme.textDim;
    ctx.fillText(`${snapshot.instrument}  ·  Bid × Ask`, PADDING.left + 4, 13);

    ctx.textAlign = "right";
    ctx.fillStyle = theme.muted;
    const state = this.isLive ? "LIVE" : `-${this.scrollBack} bars`;
    ctx.fillText(`${visible.length} bars  ·  ${state}`, width - PADDING.right - 4, 13);
  }
}
