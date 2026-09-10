/**
 * Mirrors of the DTOs in `crates/app/src/dto.rs`.
 *
 * Every price and quantity arrives as a fixed-point **minor-unit integer**,
 * never a decimal. JavaScript numbers are IEEE doubles and hold integers
 * exactly below 2^53 (~9.0e15); minor units at 8 decimal places put a
 * six-figure price near 9.5e12, well inside that. Divide by `scale` only when
 * formatting or positioning — never compare decimals for equality, which is
 * the whole reason the Rust side is fixed point.
 */

/** A price or quantity in minor units. */
export type Minor = number;

export interface ClusterDto {
  price: Minor;
  bid: Minor;
  ask: Minor;
  trades: number;
}

export interface ImbalanceDto {
  price: Minor;
  side: "buy" | "sell";
  ratio: number | null;
}

export interface BarDto {
  open_ts: number;
  close_ts: number;
  open: Minor;
  high: Minor;
  low: Minor;
  close: Minor;
  volume: Minor;
  delta: Minor;
  trades: number;
  closed: boolean;
  poc: Minor | null;
  clusters: ClusterDto[];
  imbalances: ImbalanceDto[];
}

export interface LevelDto {
  price: Minor;
  qty: Minor;
}

export interface BookDto {
  bids: LevelDto[];
  asks: LevelDto[];
  spread: Minor | null;
  crossed: boolean;
}

export interface TapeRowDto {
  ts: number;
  price: Minor;
  qty: Minor;
  side: "buy" | "sell";
}

export interface IndicatorsDto {
  cvd: Minor;
  divergence: "bearish" | "bullish" | null;
  vwap: Minor | null;
  vwap_deviation: Minor | null;
  session_poc: Minor | null;
  value_area_high: Minor | null;
  value_area_low: Minor | null;
  speed_of_tape: number;
}

export interface PositionDto {
  qty: Minor;
  avg_price: Minor;
  realised: Minor;
  unrealised: Minor;
  commission: Minor;
  side: "buy" | "sell" | null;
}

export interface OrderDto {
  id: number;
  side: "buy" | "sell";
  kind: "market" | "limit" | "stop" | "stop_limit";
  qty: Minor;
  filled: Minor;
  limit_price: Minor | null;
  trigger_price: Minor | null;
  status:
    | "pending"
    | "working"
    | "partially_filled"
    | "filled"
    | "cancelled"
    | "rejected";
}

export interface FillDto {
  order_id: number;
  ts: number;
  price: Minor;
  qty: Minor;
  side: "buy" | "sell";
}

export interface SnapshotDto {
  instrument: string;
  scale: number;
  /** The instrument's minimum price increment. */
  tick_size: Minor;
  /** Height of one footprint row. May span several instrument ticks. */
  row_size: Minor;
  /** Instrument ticks per footprint row. */
  ticks_per_row: number;
  price_decimals: number;
  bars: BarDto[];
  book: BookDto;
  tape: TapeRowDto[];
  indicators: IndicatorsDto;
  position: PositionDto;
  orders: OrderDto[];
  last_price: Minor | null;
  last_ts: number;
}

/** Tagged union matching `AppEvent`'s serde representation. */
export type AppEvent =
  | { type: "bar_closed"; 0: BarDto }
  | { type: "bar_updated"; 0: BarDto }
  | { type: "book_updated"; 0: BookDto }
  | { type: "tape_row"; 0: TapeRowDto }
  | { type: "big_trade"; ts: number; price: Minor; qty: Minor; side: "buy" | "sell" }
  | { type: "scanner_hit"; open_ts: number; volume: Minor; delta: Minor; stacks: number }
  | { type: "filled"; 0: FillDto }
  | { type: "position_changed"; 0: PositionDto }
  | { type: "connection_changed"; connected: boolean; detail: string };

/** Convert minor units to a display number. Formatting only. */
export function toDisplay(minor: Minor, scale: number): number {
  return minor / scale;
}

/** Format minor units with a fixed number of decimal places. */
export function formatMinor(
  minor: Minor,
  scale: number,
  decimals: number,
): string {
  return (minor / scale).toFixed(decimals);
}

/**
 * Compact volume rendering: 1234 becomes "1.2k".
 *
 * Zero renders as a dot, not "0.000". Half a footprint's cells have nothing on
 * one side, and printing a precise zero for each of them turns the ladder into
 * noise that hides the numbers that matter.
 */
export function formatVolume(minor: Minor, scale: number): string {
  if (minor === 0) return "·";
  const value = Math.abs(minor / scale);
  const sign = minor < 0 ? "-" : "";
  if (value >= 1_000_000) return `${sign}${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${sign}${(value / 1_000).toFixed(1)}k`;
  if (value >= 10) return `${sign}${value.toFixed(0)}`;
  return `${sign}${value.toFixed(value < 1 ? 3 : 1)}`;
}
