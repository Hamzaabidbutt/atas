import type { AppEvent, SnapshotDto } from "./types";

/**
 * Where the UI gets its data.
 *
 * The app never talks to Tauri directly. That keeps the panes runnable in a
 * plain browser against {@link MockTransport}, which is how they get developed
 * and screenshot-tested — a renderer you can only exercise by launching the
 * desktop binary is a renderer that stops being checked.
 */
export interface Transport {
  /** Full frame, for first paint and after a reconnect. */
  snapshot(): Promise<SnapshotDto>;
  /** Subscribe to incremental updates. Returns an unsubscribe function. */
  subscribe(handler: (event: AppEvent) => void): () => void;
  /** Send an order command. */
  command(name: string, payload?: unknown): Promise<unknown>;
  /** Human-readable source name, shown in the status bar. */
  readonly label: string;
}

interface TauriGlobal {
  core: { invoke(cmd: string, args?: unknown): Promise<unknown> };
  event: {
    listen(
      name: string,
      handler: (e: { payload: unknown }) => void,
    ): Promise<() => void>;
  };
}

function tauri(): TauriGlobal | undefined {
  return (globalThis as { __TAURI__?: TauriGlobal }).__TAURI__;
}

/** True when running inside the Tauri shell. */
export function hasTauri(): boolean {
  return tauri() !== undefined;
}

/** Talks to the Rust session over Tauri's command and event bridge. */
export class TauriTransport implements Transport {
  readonly label = "tauri";

  async snapshot(): Promise<SnapshotDto> {
    const api = tauri();
    if (!api) throw new Error("Tauri bridge unavailable");
    return (await api.core.invoke("session_snapshot")) as SnapshotDto;
  }

  subscribe(handler: (event: AppEvent) => void): () => void {
    const api = tauri();
    if (!api) throw new Error("Tauri bridge unavailable");

    let stop: (() => void) | undefined;
    let cancelled = false;
    void api.event
      .listen("session://event", (e) => handler(e.payload as AppEvent))
      .then((unlisten) => {
        // The listener may resolve after the caller already unsubscribed.
        if (cancelled) unlisten();
        else stop = unlisten;
      });

    return () => {
      cancelled = true;
      stop?.();
    };
  }

  async command(name: string, payload?: unknown): Promise<unknown> {
    const api = tauri();
    if (!api) throw new Error("Tauri bridge unavailable");
    return api.core.invoke(name, payload as Record<string, unknown>);
  }
}
