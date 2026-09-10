# atas.net — front-end clone

A static, dependency-free replica of an order-flow trading platform's marketing
site, built as a design/development exercise.

> **Not affiliated with ATAS.** This is an independent, non-commercial front-end
> replica for practice. No product, service or subscription is offered here, and
> every price, statistic, quote and market figure on the site is an illustrative
> placeholder. The real site is at atas.net.

## Pages

| File | Contents |
| --- | --- |
| `index.html` | Hero with a live-animated cluster chart and tape, feature grid, cluster/DOM deep dives, markets tabs, indicator library, pricing preview, testimonials, FAQ |
| `features.html` | Platform tour: cluster charts, Market Profile & TPO, Smart DOM, Smart Tape / Big Trades / Cluster Search, indicator library, automation |
| `pricing.html` | Four plans with a monthly/yearly toggle, full feature comparison table, billing FAQ |
| `download.html` | Install steps, system requirements, data connections, post-install FAQ |

## Stack

Plain HTML, CSS and vanilla JavaScript — no build step, no framework, no runtime
dependencies. Fonts come from Google Fonts; everything else is local.

```
assets/
  css/styles.css      design tokens + all components
  js/main.js          nav, scroll reveal, tabs, pricing toggle, animated tape
  js/footprint.js     canvas cluster/footprint chart in the hero
  img/                logo, favicon, social image (SVG)
```

### The hero chart

`assets/js/footprint.js` renders a synthetic footprint chart on a `<canvas>`:
candles split into per-price bid × ask cells, with imbalance shading, per-bar
POC outlining and a delta footer. Bars are generated from a seeded PRNG so the
chart looks the same on every load, and it scrolls a new bar in roughly every
four seconds. It pauses when the tab is hidden or the canvas scrolls out of
view, and does not animate at all under `prefers-reduced-motion`.

## Running it

Any static server works:

```bash
python3 -m http.server 8000
# then open http://localhost:8000
```

Or open `index.html` directly in a browser.

## Notes

- Responsive down to ~360px; tables and the cluster grid scroll horizontally
  rather than breaking the layout.
- Dark theme only, matching the source design.
- Tabs and the mobile nav are keyboard-navigable; `prefers-reduced-motion` is
  respected throughout.
