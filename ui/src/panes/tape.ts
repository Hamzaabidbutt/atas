import type { SnapshotDto, TapeRowDto } from "../types";
import { formatMinor, formatVolume } from "../types";
import { monoSmall, prepare, theme } from "../theme";

/**
 * Time and sales.
 *
 * Prints above the highlight threshold get a background, because the point of
 * watching the tape is catching the size that moves things, not reading every
 * one-lot.
 */
export class TapePane {
  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly highlightAbove = 150,
  ) {}

  render(snapshot: SnapshotDto, rows: TapeRowDto[]): void {
    const surface = prepare(this.canvas);
    if (!surface) return;
    const { ctx, width, height } = surface;

    ctx.fillStyle = theme.panel;
    ctx.fillRect(0, 0, width, height);

    const rowHeight = 15;
    const visible = Math.floor((height - 18) / rowHeight);
    // Newest first: the top of the tape is where the eye goes.
    const shown = rows.slice(-visible).reverse();

    ctx.font = monoSmall;
    ctx.textBaseline = "middle";
    ctx.fillStyle = theme.muted;
    ctx.textAlign = "left";
    ctx.fillText("PRICE", 8, 10);
    ctx.textAlign = "right";
    ctx.fillText("SIZE", width - 52, 10);
    ctx.fillText("TIME", width - 8, 10);

    ctx.strokeStyle = theme.grid;
    ctx.beginPath();
    ctx.moveTo(0, 18.5);
    ctx.lineTo(width, 18.5);
    ctx.stroke();

    shown.forEach((row, i) => {
      const y = 18 + i * rowHeight;
      const mid = y + rowHeight / 2;
      const size = row.qty / snapshot.scale;

      if (size >= this.highlightAbove) {
        ctx.fillStyle = "rgba(255,176,32,0.13)";
        ctx.fillRect(0, y, width, rowHeight);
      }

      const colour = row.side === "buy" ? theme.buy : theme.sell;
      ctx.fillStyle = colour;
      ctx.textAlign = "left";
      ctx.fillText(
        formatMinor(row.price, snapshot.scale, snapshot.price_decimals),
        8,
        mid,
      );

      ctx.fillStyle = size >= this.highlightAbove ? theme.poc : colour;
      ctx.textAlign = "right";
      ctx.fillText(formatVolume(row.qty, snapshot.scale), width - 52, mid);

      ctx.fillStyle = theme.muted;
      ctx.fillText(clockOf(row.ts), width - 8, mid);
    });
  }
}

/** Nanosecond epoch to `mm:ss`. */
function clockOf(nanos: number): string {
  const date = new Date(nanos / 1_000_000);
  const mm = String(date.getMinutes()).padStart(2, "0");
  const ss = String(date.getSeconds()).padStart(2, "0");
  return `${mm}:${ss}`;
}
