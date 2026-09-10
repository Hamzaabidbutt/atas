/** Colours shared by every canvas pane. Kept in one place so the DOM ladder,
 *  footprint and tape agree on what "buy" looks like. */
export const theme = {
  bg: "#07090e",
  panel: "#0d1119",
  grid: "rgba(255,255,255,0.04)",
  gridStrong: "rgba(255,255,255,0.09)",
  text: "#e9eef8",
  textDim: "#8b98af",
  muted: "#5d6a80",
  buy: "#21e0a1",
  sell: "#ff4d68",
  buyFill: "rgba(33,224,161,0.16)",
  sellFill: "rgba(255,77,104,0.16)",
  buyHot: "rgba(33,224,161,0.42)",
  sellHot: "rgba(255,77,104,0.42)",
  poc: "#ffb020",
  vwap: "#4d8dff",
  cell: "rgba(255,255,255,0.03)",
  accent: "#21e0a1",
} as const;

export const mono =
  '11px "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';
export const monoSmall =
  '9px "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';

/** Prepare a canvas for crisp drawing at the device pixel ratio.
 *  Returns null when the element has no layout yet. */
export function prepare(
  canvas: HTMLCanvasElement,
): { ctx: CanvasRenderingContext2D; width: number; height: number } | null {
  const rect = canvas.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) return null;

  const dpr = Math.min(globalThis.devicePixelRatio || 1, 2);
  const width = Math.round(rect.width);
  const height = Math.round(rect.height);
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);

  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  return { ctx, width, height };
}
