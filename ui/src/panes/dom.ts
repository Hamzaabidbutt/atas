import type { SnapshotDto } from "../types";
import { formatMinor, formatVolume } from "../types";
import { monoSmall, prepare, theme } from "../theme";

/**
 * The depth-of-market ladder.
 *
 * Resting size is drawn as bars growing outward from the price column, so
 * relative depth reads at a glance rather than by comparing numbers. Bids and
 * asks occupy separate columns and never share a row: a ladder that stacks
 * them invites reading a bid as an offer, which is a mistake with a cost.
 */
export class DomLadder {
  constructor(private readonly canvas: HTMLCanvasElement) {}

  render(snapshot: SnapshotDto): void {
    const surface = prepare(this.canvas);
    if (!surface) return;
    const { ctx, width, height } = surface;

    ctx.fillStyle = theme.panel;
    ctx.fillRect(0, 0, width, height);

    const { bids, asks } = snapshot.book;
    if (bids.length === 0 && asks.length === 0) {
      ctx.fillStyle = theme.muted;
      ctx.font = monoSmall;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText("no depth", width / 2, height / 2);
      return;
    }

    // Asks descend to the touch, then bids descend away from it, so the
    // spread sits in the middle exactly as a trader expects to see it.
    const rows = [
      ...[...asks].reverse().map((l) => ({ ...l, side: "ask" as const })),
      ...bids.map((l) => ({ ...l, side: "bid" as const })),
    ];

    const rowHeight = Math.min(18, Math.max(9, height / Math.max(rows.length, 1)));
    const priceColumn = 74;
    const barWidth = Math.max(10, (width - priceColumn) / 2 - 6);
    const centre = width / 2;

    let peak = 1;
    for (const row of rows) peak = Math.max(peak, row.qty);

    ctx.font = monoSmall;
    ctx.textBaseline = "middle";

    rows.forEach((row, i) => {
      const y = i * rowHeight;
      if (y > height) return;
      const mid = y + rowHeight / 2;
      const scaled = (row.qty / peak) * barWidth;

      if (i % 2 === 1) {
        ctx.fillStyle = "rgba(255,255,255,0.015)";
        ctx.fillRect(0, y, width, rowHeight);
      }

      if (row.side === "bid") {
        ctx.fillStyle = "rgba(33,224,161,0.28)";
        ctx.fillRect(centre - priceColumn / 2 - scaled, y + 1, scaled, rowHeight - 2);
        ctx.fillStyle = theme.buy;
        ctx.textAlign = "right";
        ctx.fillText(formatVolume(row.qty, snapshot.scale), centre - priceColumn / 2 - 5, mid);
      } else {
        ctx.fillStyle = "rgba(255,77,104,0.28)";
        ctx.fillRect(centre + priceColumn / 2, y + 1, scaled, rowHeight - 2);
        ctx.fillStyle = theme.sell;
        ctx.textAlign = "left";
        ctx.fillText(formatVolume(row.qty, snapshot.scale), centre + priceColumn / 2 + 5, mid);
      }

      ctx.fillStyle = theme.textDim;
      ctx.textAlign = "center";
      ctx.fillText(
        formatMinor(row.price, snapshot.scale, snapshot.price_decimals),
        centre,
        mid,
      );
    });

    // A crossed book means the feed is inconsistent; say so rather than
    // letting a trader act on it.
    if (snapshot.book.crossed) {
      ctx.fillStyle = theme.sell;
      ctx.textAlign = "center";
      ctx.fillText("CROSSED BOOK", width / 2, height - 8);
    }
  }
}
