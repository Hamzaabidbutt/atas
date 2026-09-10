/**
 * Rendering smoke test.
 *
 * Canvas panes have no DOM to assert against, so a unit test cannot tell a
 * working renderer from one that silently draws nothing. This loads the built
 * app, samples the actual pixels of each canvas, and fails if a pane came back
 * blank — which is the failure mode that matters and the one that is otherwise
 * invisible until someone opens the app.
 *
 *   node tests/render.mjs [--url http://127.0.0.1:8899] [--out shots]
 *
 * Requires a browser: `npx playwright install chromium`, or set
 * PLAYWRIGHT_CHROMIUM to an existing binary.
 */
import { chromium } from "playwright";
import { mkdir } from "node:fs/promises";

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};

const url = flag("url", "http://127.0.0.1:8899/");
const outDir = flag("out", "shots");
const PANES = ["chart", "dom", "tape"];
/** Minimum lit pixel samples before a pane counts as having drawn. */
const MIN_LIT = 40;

await mkdir(outDir, { recursive: true });

const launch = process.env.PLAYWRIGHT_CHROMIUM
  ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM }
  : {};
const browser = await chromium.launch(launch);

const problems = [];
try {
  for (const [label, width, height] of [
    ["desktop", 1600, 900],
    ["narrow", 900, 720],
  ]) {
    const context = await browser.newContext({ viewport: { width, height } });
    const page = await context.newPage();

    page.on("console", (m) => {
      if (m.type() === "error") problems.push(`${label} console: ${m.text()}`);
    });
    page.on("pageerror", (e) => problems.push(`${label} pageerror: ${e.message}`));
    page.on("requestfailed", (r) =>
      problems.push(`${label} request failed: ${r.url()}`),
    );

    await page.goto(url, { waitUntil: "networkidle" });
    // Let the mock feed produce a few frames.
    await page.waitForTimeout(2000);

    const report = await page.evaluate((panes) => {
      const result = {};
      for (const id of panes) {
        const canvas = document.getElementById(id);
        if (!(canvas instanceof HTMLCanvasElement)) {
          result[id] = { missing: true };
          continue;
        }
        const ctx = canvas.getContext("2d");
        const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height);
        let lit = 0;
        // Sample sparsely; a full scan of a retina canvas is needlessly slow.
        for (let i = 0; i < data.length; i += 4 * 97) {
          if (data[i] > 40 || data[i + 1] > 40 || data[i + 2] > 40) lit++;
        }
        result[id] = { width: canvas.width, height: canvas.height, lit };
      }
      result.stats = document.querySelectorAll(".stat-value").length;
      result.overflow =
        document.documentElement.scrollWidth >
        document.documentElement.clientWidth + 1;
      return result;
    }, PANES);

    for (const id of PANES) {
      const pane = report[id];
      if (pane?.missing) problems.push(`${label}: #${id} is not a canvas`);
      else if (!pane || pane.lit < MIN_LIT) {
        problems.push(`${label}: #${id} drew nothing (lit=${pane?.lit ?? 0})`);
      }
    }
    if (report.stats < 1) problems.push(`${label}: status strip is empty`);
    if (report.overflow) problems.push(`${label}: page scrolls horizontally`);

    await page.screenshot({ path: `${outDir}/${label}.png` });
    console.log(`${label}:`, JSON.stringify(report));
    await context.close();
  }
} finally {
  await browser.close();
}

if (problems.length > 0) {
  console.error("\nFAILED:\n" + problems.join("\n"));
  process.exit(1);
}
console.log("\nAll panes rendered.");
