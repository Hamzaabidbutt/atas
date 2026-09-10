import { MockTransport } from "./data/generator";
import { DomLadder } from "./panes/dom";
import { FootprintChart } from "./panes/footprint";
import { StatusStrip } from "./panes/status";
import { TapePane } from "./panes/tape";
import { hasTauri, TauriTransport, type Transport } from "./transport";
import type { AppEvent, SnapshotDto, TapeRowDto } from "./types";

/**
 * Wires the panes to a transport and keeps one authoritative snapshot.
 *
 * Incremental events mutate that snapshot rather than each pane keeping its
 * own copy: two panes with independent state drift apart the moment one
 * misses an event, and the drift is invisible until someone trades on it.
 */
class App {
  private snapshot: SnapshotDto | undefined;
  private tape: TapeRowDto[] = [];
  private dirty = true;
  private frame = 0;

  private readonly chart: FootprintChart;
  private readonly ladder: DomLadder;
  private readonly tapePane: TapePane;
  private readonly status: StatusStrip;

  constructor(private readonly transport: Transport) {
    this.chart = new FootprintChart(byId<HTMLCanvasElement>("chart"));
    this.ladder = new DomLadder(byId<HTMLCanvasElement>("dom"));
    this.tapePane = new TapePane(byId<HTMLCanvasElement>("tape"));
    this.status = new StatusStrip(byId<HTMLElement>("status"));

    byId<HTMLElement>("source").textContent = transport.label;
  }

  async start(): Promise<void> {
    this.snapshot = await this.transport.snapshot();
    this.tape = [...this.snapshot.tape];
    this.dirty = true;

    this.transport.subscribe((event) => this.apply(event));

    // Redraw on a frame, not per event: a busy instrument prints far faster
    // than a display refreshes, and drawing per print burns the CPU a
    // renderer needs to stay responsive.
    const loop = () => {
      if (this.dirty) {
        this.draw();
        this.dirty = false;
      }
      this.frame = requestAnimationFrame(loop);
    };
    this.frame = requestAnimationFrame(loop);

    globalThis.addEventListener("resize", () => {
      this.dirty = true;
    });

    this.bindControls();
  }

  stop(): void {
    cancelAnimationFrame(this.frame);
  }

  private apply(event: AppEvent): void {
    const snapshot = this.snapshot;
    if (!snapshot) return;

    switch (event.type) {
      case "bar_closed": {
        const bar = event[0];
        // The forming bar already occupies the last slot; closing replaces it.
        const last = snapshot.bars.at(-1);
        if (last && !last.closed) snapshot.bars[snapshot.bars.length - 1] = bar;
        else snapshot.bars.push(bar);
        if (snapshot.bars.length > 500) snapshot.bars.shift();
        break;
      }
      case "bar_updated": {
        const bar = event[0];
        const last = snapshot.bars.at(-1);
        if (last && !last.closed) snapshot.bars[snapshot.bars.length - 1] = bar;
        else snapshot.bars.push(bar);
        break;
      }
      case "book_updated":
        snapshot.book = event[0];
        break;
      case "tape_row": {
        const row = event[0];
        this.tape.push(row);
        if (this.tape.length > 200) this.tape.shift();
        snapshot.last_price = row.price;
        snapshot.last_ts = row.ts;
        break;
      }
      case "position_changed":
        snapshot.position = event[0];
        break;
      case "filled":
        break;
      case "big_trade":
        break;
      case "scanner_hit":
        this.flashAlert(`Scanner hit · ${event.stacks} stacked imbalance runs`);
        break;
      case "connection_changed":
        byId<HTMLElement>("connection").textContent = event.connected
          ? "connected"
          : `disconnected — ${event.detail}`;
        byId<HTMLElement>("connection").classList.toggle("down", !event.connected);
        break;
    }
    this.dirty = true;
  }

  private draw(): void {
    if (!this.snapshot) return;
    this.chart.render(this.snapshot);
    this.ladder.render(this.snapshot);
    this.tapePane.render(this.snapshot, this.tape);
    this.status.render(this.snapshot);
  }

  private flashAlert(text: string): void {
    const element = byId<HTMLElement>("alert");
    element.textContent = text;
    element.classList.add("visible");
    setTimeout(() => element.classList.remove("visible"), 4000);
  }

  private bindControls(): void {
    byId<HTMLButtonElement>("scroll-back").addEventListener("click", () => {
      this.chart.scrollBy(5);
      this.dirty = true;
    });
    byId<HTMLButtonElement>("scroll-forward").addEventListener("click", () => {
      this.chart.scrollBy(-5);
      this.dirty = true;
    });
    byId<HTMLButtonElement>("scroll-live").addEventListener("click", () => {
      this.chart.scrollToLive();
      this.dirty = true;
    });

    for (const [id, command] of [
      ["buy", "buy_market"],
      ["sell", "sell_market"],
      ["flatten", "flatten"],
    ] as const) {
      byId<HTMLButtonElement>(id).addEventListener("click", () => {
        void this.transport.command(command, { qty: 1 });
      });
    }
  }
}

function byId<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing element #${id}`);
  return element as T;
}

// In the desktop shell this talks to the Rust session; in a plain browser it
// falls back to the mock so the renderers stay developable and testable.
const transport: Transport = hasTauri() ? new TauriTransport() : new MockTransport();
void new App(transport).start();
