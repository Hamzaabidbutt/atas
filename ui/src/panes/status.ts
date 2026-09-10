import type { SnapshotDto } from "../types";
import { formatMinor, formatVolume } from "../types";

/**
 * The readout strip: indicators and position, as DOM rather than canvas.
 *
 * Text that a trader may want to select, copy or have read out by a screen
 * reader has no business being pixels.
 */
export class StatusStrip {
  private readonly fields = new Map<string, HTMLElement>();

  constructor(private readonly root: HTMLElement) {
    const layout: Array<[string, string]> = [
      ["last", "Last"],
      ["cvd", "CVD"],
      ["vwap", "VWAP"],
      ["poc", "Session POC"],
      ["speed", "Tape/s"],
      ["position", "Position"],
      ["pnl", "PnL"],
    ];

    for (const [key, label] of layout) {
      const cell = document.createElement("div");
      cell.className = "stat";
      const name = document.createElement("span");
      name.className = "stat-label";
      name.textContent = label;
      const value = document.createElement("b");
      value.className = "stat-value";
      value.textContent = "—";
      cell.append(name, value);
      this.root.append(cell);
      this.fields.set(key, value);
    }
  }

  private set(key: string, text: string, tone?: "up" | "down"): void {
    const element = this.fields.get(key);
    if (!element) return;
    element.textContent = text;
    element.classList.toggle("up", tone === "up");
    element.classList.toggle("down", tone === "down");
  }

  render(snapshot: SnapshotDto): void {
    const { scale, price_decimals: decimals, indicators, position } = snapshot;

    this.set(
      "last",
      snapshot.last_price === null
        ? "—"
        : formatMinor(snapshot.last_price, scale, decimals),
    );

    this.set(
      "cvd",
      formatVolume(indicators.cvd, scale),
      indicators.cvd >= 0 ? "up" : "down",
    );

    this.set(
      "vwap",
      indicators.vwap === null ? "—" : formatMinor(indicators.vwap, scale, decimals),
    );

    this.set(
      "poc",
      indicators.session_poc === null
        ? "—"
        : formatMinor(indicators.session_poc, scale, decimals),
    );

    this.set("speed", indicators.speed_of_tape.toFixed(1));

    if (position.side === null) {
      this.set("position", "flat");
    } else {
      const size = formatVolume(Math.abs(position.qty), scale);
      const at = formatMinor(position.avg_price, scale, decimals);
      this.set(
        "position",
        `${position.side === "buy" ? "long" : "short"} ${size} @ ${at}`,
        position.side === "buy" ? "up" : "down",
      );
    }

    const total = position.realised + position.unrealised - position.commission;
    this.set("pnl", formatVolume(total, scale), total >= 0 ? "up" : "down");
  }
}
