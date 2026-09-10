/* ATAS clone — canvas footprint / cluster chart for the hero terminal.
   Renders synthetic order-flow bars: candle body + per-price bid x ask cells,
   with imbalance and POC highlighting, scrolling left as new bars print. */
(function () {
  'use strict';

  var canvas = document.getElementById('footprint');
  if (!canvas) return;

  var ctx = canvas.getContext('2d');
  var reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  var TICK = 0.25;          // price increment per cluster row
  var COL_W = 74;           // bar width in CSS px
  var ROW_H = 15;           // cluster row height in CSS px
  var PAD = { t: 34, r: 54, b: 24, l: 12 };

  var COLORS = {
    grid:    'rgba(255,255,255,0.035)',
    axis:    '#7d8ba3',
    up:      '#21e0a1',
    down:    '#ff4d68',
    upFill:  'rgba(33,224,161,0.16)',
    dnFill:  'rgba(255,77,104,0.16)',
    cellBg:  'rgba(255,255,255,0.028)',
    hotBuy:  'rgba(33,224,161,0.30)',
    hotSell: 'rgba(255,77,104,0.30)',
    poc:     '#ffb020',
    text:    'rgba(233,238,248,0.78)',
    dim:     'rgba(180,192,212,0.55)'
  };

  /* --- Seeded PRNG so the chart looks the same on every load --------------- */
  var seed = 20240917;
  function rnd() {
    seed = (seed * 1664525 + 1013904223) % 4294967296;
    return seed / 4294967296;
  }

  var bars = [];
  var last = 5312.5;

  var ANCHOR = 5312.5;

  function makeBar() {
    var open = last;
    // mean-reverting walk: keeps the visible band tight enough that every
    // bar's clusters stay on screen, while still trending in short runs
    var reversion = (ANCHOR - open) * 0.26;
    var shock = (rnd() - 0.5) * 2.9;
    var close = open + reversion + shock;
    var dir = close >= open ? 1 : -1;
    var body = Math.abs(close - open);
    var wick = 0.25 + rnd() * 0.9;

    var high = Math.max(open, close) + wick * rnd();
    var low = Math.min(open, close) - wick * rnd();

    high = Math.ceil(high / TICK) * TICK;
    low = Math.floor(low / TICK) * TICK;
    // clamp the number of cluster rows so a wide bar cannot swamp the pane
    while ((high - low) / TICK > 15) { high -= TICK; low += TICK; }
    last = close;

    var rows = [];
    var levels = Math.round((high - low) / TICK) + 1;
    var mid = (levels - 1) / 2;
    var pocIndex = 0;
    var pocVol = -1;

    for (var i = 0; i < levels; i++) {
      // volume is fattest in the middle of the bar's range
      var shape = 1 - Math.abs(i - mid) / (mid + 1);
      var base = 40 + shape * (420 + body * 90) * (0.55 + rnd() * 0.75);
      var skew = dir > 0 ? 0.56 + rnd() * 0.2 : 0.24 + rnd() * 0.2;
      var bid = Math.round(base * (1 - skew));
      var ask = Math.round(base * skew);
      var total = bid + ask;
      if (total > pocVol) { pocVol = total; pocIndex = i; }
      rows.push({ price: low + i * TICK, bid: bid, ask: ask });
    }

    return {
      open: open, close: close, high: high, low: low,
      up: close >= open, rows: rows, poc: pocIndex
    };
  }

  function fmt(n) { return n.toFixed(2); }

  /* --- Layout / draw ------------------------------------------------------ */
  var W = 0, H = 0, cols = 0, dpr = 1;

  function resize() {
    var rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height) return false;
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    W = rect.width;
    H = rect.height;
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    cols = Math.max(3, Math.ceil((W - PAD.l - PAD.r) / COL_W) + 1);
    while (bars.length < cols) bars.push(makeBar());
    while (bars.length > cols) bars.shift();
    return true;
  }

  function draw(shift) {
    if (!W || !H) return;
    ctx.clearRect(0, 0, W, H);

    var plotH = H - PAD.t - PAD.b;
    var visible = bars;

    var hi = -Infinity, lo = Infinity;
    visible.forEach(function (b) {
      if (b.high > hi) hi = b.high;
      if (b.low < lo) lo = b.low;
    });
    var span = Math.max(hi - lo, TICK * 8);
    hi += span * 0.06;
    lo -= span * 0.06;
    span = hi - lo;

    var y = function (p) { return PAD.t + (hi - p) / span * plotH; };

    /* horizontal grid + right price axis */
    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.textBaseline = 'middle';
    var stepCount = 6;
    for (var g = 0; g <= stepCount; g++) {
      var p = lo + (span / stepCount) * g;
      var gy = Math.round(y(p)) + 0.5;
      ctx.strokeStyle = COLORS.grid;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(PAD.l, gy);
      ctx.lineTo(W - PAD.r, gy);
      ctx.stroke();
      ctx.fillStyle = COLORS.dim;
      ctx.textAlign = 'left';
      ctx.fillText(fmt(p), W - PAD.r + 8, gy);
    }

    var cellH = Math.max(9, Math.min(ROW_H, plotH / 26));

    ctx.save();
    ctx.beginPath();
    ctx.rect(PAD.l, 0, W - PAD.r - PAD.l, H);
    ctx.clip();

    visible.forEach(function (bar, i) {
      var x = PAD.l + (i - 1) * COL_W - shift;
      var colColor = bar.up ? COLORS.up : COLORS.down;

      /* wick */
      ctx.strokeStyle = colColor;
      ctx.globalAlpha = 0.55;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(Math.round(x + 4) + 0.5, y(bar.high));
      ctx.lineTo(Math.round(x + 4) + 0.5, y(bar.low));
      ctx.stroke();
      ctx.globalAlpha = 1;

      /* body outline behind the clusters */
      var bodyTop = y(Math.max(bar.open, bar.close));
      var bodyBot = y(Math.min(bar.open, bar.close));
      ctx.fillStyle = bar.up ? COLORS.upFill : COLORS.dnFill;
      ctx.fillRect(x + 8, bodyTop, COL_W - 18, Math.max(2, bodyBot - bodyTop));

      /* cluster cells */
      var maxVol = 1;
      bar.rows.forEach(function (r) { maxVol = Math.max(maxVol, r.bid + r.ask); });

      bar.rows.forEach(function (r, ri) {
        var cy = y(r.price) - cellH / 2;
        if (cy < PAD.t - cellH || cy > H - PAD.b) return;

        var w = COL_W - 18;
        var strength = (r.bid + r.ask) / maxVol;

        ctx.fillStyle = COLORS.cellBg;
        ctx.fillRect(x + 8, cy, w, cellH - 1.5);

        /* imbalance highlighting: a side ≥ 2.4x the other is "hot" */
        if (r.ask > r.bid * 2.4) ctx.fillStyle = COLORS.hotBuy;
        else if (r.bid > r.ask * 2.4) ctx.fillStyle = COLORS.hotSell;
        else ctx.fillStyle = 'rgba(255,255,255,' + (0.02 + strength * 0.05).toFixed(3) + ')';
        ctx.fillRect(x + 8, cy, w, cellH - 1.5);

        if (ri === bar.poc) {
          ctx.strokeStyle = COLORS.poc;
          ctx.globalAlpha = 0.75;
          ctx.lineWidth = 1;
          ctx.strokeRect(x + 8.5, cy + 0.5, w - 1, cellH - 2.5);
          ctx.globalAlpha = 1;
        }

        if (cellH >= 11) {
          ctx.font = '9px ui-monospace, SFMono-Regular, Menlo, monospace';
          ctx.fillStyle = COLORS.down;
          ctx.textAlign = 'right';
          ctx.fillText(String(r.bid), x + 8 + w / 2 - 4, cy + cellH / 2 - 0.5);
          ctx.fillStyle = COLORS.up;
          ctx.textAlign = 'left';
          ctx.fillText(String(r.ask), x + 8 + w / 2 + 4, cy + cellH / 2 - 0.5);
        }
      });

      /* delta label under the bar */
      var delta = bar.rows.reduce(function (s, r) { return s + (r.ask - r.bid); }, 0);
      ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
      ctx.textAlign = 'center';
      ctx.fillStyle = delta >= 0 ? COLORS.up : COLORS.down;
      ctx.fillText((delta >= 0 ? '+' : '') + delta, x + COL_W / 2 - 1, H - PAD.b + 10);
    });

    ctx.restore();

    /* last price marker */
    var lastBar = visible[visible.length - 1];
    if (lastBar) {
      var ly = Math.round(y(lastBar.close)) + 0.5;
      ctx.setLineDash([3, 4]);
      ctx.strokeStyle = lastBar.up ? COLORS.up : COLORS.down;
      ctx.globalAlpha = 0.7;
      ctx.beginPath();
      ctx.moveTo(PAD.l, ly);
      ctx.lineTo(W - PAD.r, ly);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;

      var label = fmt(lastBar.close);
      ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
      var tw = ctx.measureText(label).width + 12;
      ctx.fillStyle = lastBar.up ? COLORS.up : COLORS.down;
      ctx.fillRect(W - PAD.r + 3, ly - 8, tw, 16);
      ctx.fillStyle = '#04150f';
      ctx.textAlign = 'left';
      ctx.fillText(label, W - PAD.r + 9, ly);
    }
  }

  /* --- Animation loop ----------------------------------------------------- */
  var offset = 0;
  var lastTs = 0;
  var raf = null;

  function frame(ts) {
    var dt = lastTs ? Math.min(ts - lastTs, 64) : 16;
    lastTs = ts;

    offset += dt * 0.02;                 // px per ms — a bar every ~3.7s
    if (offset >= COL_W) {
      offset -= COL_W;
      bars.push(makeBar());
      if (bars.length > cols) bars.shift();
    }
    draw(offset);
    raf = requestAnimationFrame(frame);
  }

  function start() {
    if (raf !== null || reduceMotion) return;
    lastTs = 0;
    raf = requestAnimationFrame(frame);
  }

  function stop() {
    if (raf !== null) { cancelAnimationFrame(raf); raf = null; }
  }

  if (!resize()) {
    // canvas not laid out yet (e.g. hidden); retry on next frame
    requestAnimationFrame(function () { if (resize()) { draw(0); start(); } });
  } else {
    draw(0);
    start();
  }

  var resizeTimer;
  window.addEventListener('resize', function () {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(function () { if (resize()) draw(offset); }, 140);
  });

  document.addEventListener('visibilitychange', function () {
    if (document.hidden) stop(); else start();
  });

  if ('IntersectionObserver' in window) {
    new IntersectionObserver(function (entries) {
      entries.forEach(function (e) { e.isIntersecting ? start() : stop(); });
    }, { threshold: 0 }).observe(canvas);
  }
})();
