/**
 * os_apps.js — RailPredict OS app view initialisers
 *
 * Exposes window.OsApps, one function per app view.
 * Each function receives the window-body root element and reads window.MAP.
 * All DOM lookups are scoped to root so multiple open windows don't collide.
 *
 * No external dependencies. Vanilla JS only.
 *
 * Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
 */
(function () {
  "use strict";

  /* ── Helpers ──────────────────────────────────────────────────────── */

  /** Escape a value for safe innerHTML insertion. */
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }

  /** Traverse dotted path into an object, returning null if any step is absent. */
  function get(obj, path) {
    return path.split(".").reduce(function (a, k) {
      return a == null ? null : a[k];
    }, obj);
  }

  /**
   * Format a value for display.
   * - null/undefined → "—"
   * - numbers: rounded to 1 dp, suffix appended
   * - strings: returned as-is, suffix appended
   */
  function fmt(v, suffix) {
    if (v == null) return "—";
    if (typeof v === "number") {
      v = Math.round(v * 10) / 10;
    }
    return suffix ? v + suffix : String(v);
  }

  /** Format a delay value as "+Nm", "0m", or "-Nm". */
  function delayVal(v) {
    if (v == null) return "—";
    v = Math.round(v);
    return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "0m";
  }

  /** Human-readable accuracy string for a delta value. */
  function accTxt(d) {
    d = Math.round(d == null ? 0 : d);
    return d === 0 ? "spot on" : "off by " + Math.abs(d) + "m";
  }

  /* ── [data-fill] KPI injection ────────────────────────────────────── */

  /**
   * Walk all [data-fill] elements within root, read the named key from
   * window.MAP, apply optional [data-suffix], and set textContent.
   */
  function fill(root) {
    var M = window.MAP || {};
    var els = root.querySelectorAll("[data-fill]");
    for (var i = 0; i < els.length; i++) {
      var el = els[i];
      var val = get(M, el.getAttribute("data-fill"));
      var sfx = el.getAttribute("data-suffix") || "";
      el.textContent = fmt(val, sfx);
    }
  }

  /* ── Operator row renderer ────────────────────────────────────────── */

  /** On-time-% band colour, shared by the operator bars. */
  function pctBand(v) { return v >= 90 ? "#34d399" : v >= 80 ? "#f2c14e" : "#f04545"; }

  /**
   * Tidy raw operator names from the feed (which arrive title-cased and
   * abbreviated, e.g. "Gwr", "Tfl Rail", "London South Eastern Railwy") into
   * recognisable brand names. Unmapped names pass through unchanged.
   */
  var OP_NAMES = {
    "scotrail": "ScotRail",
    "gwr": "GWR",
    "transport for wales rail": "Transport for Wales",
    "thameslink and gt northern tl": "Thameslink & Great Northern",
    "london south eastern railwy": "Southeastern",
    "tfl rail": "Elizabeth line",
    "lner": "LNER",
    "crosscountry": "CrossCountry",
    "c2c": "c2c",
    "emr": "East Midlands Railway",
    "tfl": "Transport for London"
  };
  function displayOperator(name) {
    if (name == null) return "Unknown";
    return OP_NAMES[String(name).toLowerCase().trim()] || name;
  }

  /**
   * Render operators as a ranked bar league: rank · name · on-time-% bar · % · journeys,
   * sorted best-first. Accepts either the day-asset shape ({name, otp, j}) or the
   * baked OsData shape ({name, on_time_pct, journeys}).
   *
   * @param {Element} container
   * @param {Array}   operators
   * @param {number}  [maxRows]  - cap rows (default unlimited)
   */
  function renderOperatorLeague(container, operators, maxRows) {
    if (!container) return;
    var list = (operators || []).map(function (o) {
      return {
        name: displayOperator(o.name),
        otp: (o.otp != null ? o.otp : o.on_time_pct),
        journeys: (o.journeys != null ? o.journeys : o.j)
      };
    }).filter(function (o) { return o.otp != null; });
    list.sort(function (a, b) { return (b.otp - a.otp) || ((b.journeys || 0) - (a.journeys || 0)); });
    if (maxRows) list = list.slice(0, maxRows);
    if (!list.length) {
      container.innerHTML = '<p class="empty">No operator data available.</p>';
      return;
    }
    var html = "";
    for (var i = 0; i < list.length; i++) {
      var o = list[i];
      var w = Math.max(0, Math.min(100, o.otp));
      var jn = o.journeys != null ? Number(o.journeys).toLocaleString() : "—";
      html +=
        '<div class="op-lrow' + (i < 3 ? " top" : "") + '">' +
          '<span class="op-rank">' + (i + 1) + '</span>' +
          '<span class="op-name">' + esc(o.name) + '</span>' +
          '<span class="op-bar"><span class="op-bar-fill" style="width:' + w.toFixed(1) + '%;background:' + pctBand(o.otp) + '"></span></span>' +
          '<span class="op-pct">' + (Math.round(o.otp * 10) / 10) + '%</span>' +
          '<span class="op-jn">' + esc(jn) + '</span>' +
        '</div>';
    }
    container.innerHTML = html;
  }

  /* ── Reliability viz helpers ──────────────────────────────────────── */

  /** Fill a gauge bar element to v% with the given colour. */
  function setGauge(el, v, color) {
    if (!el) return;
    var w = Math.max(0, Math.min(100, v == null ? 0 : v));
    el.style.width = w.toFixed(1) + "%";
    el.style.background = color;
  }

  /**
   * Draw the 24-hour "% of trains 5+ min late" profile. Bars scale to the
   * busiest-sampled hour; under-sampled hours (n < MIN_N) are dimmed and capped
   * short so a handful of night services can't dominate the picture.
   *
   * @param {Element} barsEl  - the 24-column bar grid
   * @param {Element} axisEl  - the hour-label axis (labels every 6h)
   * @param {Array}   hourly  - [{h, late, n}, …]
   */
  function renderHourly(barsEl, axisEl, peakEl, hourly) {
    if (!barsEl) return;
    function pad(n) { return (n < 10 ? "0" : "") + n; }
    if (!hourly || !hourly.length) {
      barsEl.innerHTML = '<p class="empty">Not enough data yet.</p>';
      if (axisEl) axisEl.innerHTML = "";
      if (peakEl) peakEl.textContent = "";
      return;
    }
    var MIN_N = 40, byH = {};
    for (var i = 0; i < hourly.length; i++) byH[hourly[i].h] = hourly[i];
    var maxLate = 1, peakHour = -1;
    for (var h = 0; h < 24; h++) { var r = byH[h]; if (r && r.n >= MIN_N && r.late > maxLate) { maxLate = r.late; peakHour = h; } }
    function band(v) { return v <= 6 ? "#34d399" : v <= 12 ? "#f2c14e" : "#f04545"; }
    var bars = "", axis = "";
    for (var hr = 0; hr < 24; hr++) {
      var rr = byH[hr], late = rr ? rr.late : 0, n = rr ? rr.n : 0, low = n < MIN_N;
      var px = Math.max(2, Math.round(late / maxLate * 84));
      if (low) px = Math.min(px, 12);
      var color = (!rr || n === 0) ? "#e5e7eb" : low ? "#cbd5e1" : band(late);
      var hh = pad(hr);
      var title = rr ? (hh + ":00 · " + late + "% late · " + n + " svc" + (low ? " (low volume)" : "")) : (hh + ":00 · no data");
      bars += '<span class="hbar" style="height:' + px + 'px;background:' + color + '" title="' + esc(title) + '"></span>';
      axis += '<span>' + (hr % 6 === 0 ? hh : "") + '</span>';
    }
    barsEl.innerHTML = bars;
    if (axisEl) axisEl.innerHTML = axis;
    if (peakEl) peakEl.textContent = peakHour >= 0 ? ("peak " + pad(peakHour) + ":00 · " + maxLate + "% late") : "";
  }

  /**
   * Signed prediction-error histogram (actual − predicted, minutes) from the
   * day's scored outcomes. Coloured by accuracy: near-zero green, a few minutes
   * amber, large miss red — so a tight, centred shape reads as accurate.
   *
   * @param {Element} el      - the bar container
   * @param {Array}   replay  - [{p, a}, …] predicted / actual delay
   */
  function renderHistogram(el, replay) {
    if (!el) return;
    if (!replay || !replay.length) { el.innerHTML = '<p class="empty">No prediction data.</p>'; return; }
    var buckets = [
      { lbl: "≤-6",    lo: -1e9, hi: -6,  c: "#f04545" },
      { lbl: "-5..-2", lo: -5,   hi: -2,  c: "#f2c14e" },
      { lbl: "-1",     lo: -1,   hi: -1,  c: "#34d399" },
      { lbl: "0",      lo: 0,    hi: 0,   c: "#16a34a" },
      { lbl: "+1",     lo: 1,    hi: 1,   c: "#34d399" },
      { lbl: "+2..+5", lo: 2,    hi: 5,   c: "#f2c14e" },
      { lbl: "≥+6",    lo: 6,    hi: 1e9, c: "#f04545" }
    ];
    var counts = buckets.map(function () { return 0; }), total = replay.length;
    for (var i = 0; i < replay.length; i++) {
      var e = replay[i].a - replay[i].p;
      for (var b = 0; b < buckets.length; b++) {
        if (e >= buckets[b].lo && e <= buckets[b].hi) { counts[b]++; break; }
      }
    }
    var max = Math.max.apply(null, counts) || 1, html = "";
    for (var k = 0; k < buckets.length; k++) {
      var pct = Math.round(1000 * counts[k] / total) / 10;
      var px = Math.max(3, Math.round(counts[k] / max * 96));
      html +=
        '<div class="hcol">' +
          '<span class="hval">' + pct + '%</span>' +
          '<span class="hbar2" style="height:' + px + 'px;background:' + buckets[k].c + '"></span>' +
          '<span class="hlbl">' + esc(buckets[k].lbl) + '</span>' +
        '</div>';
    }
    el.innerHTML = html;
  }

  /* ── Replay card builders ─────────────────────────────────────────── */

  function trackCard(t) {
    return (
      '<div class="mini" style="--c:' + esc(t.brand || "#94a3b8") + '">' +
        '<span class="hc">' + esc(t.label || "") + '</span>' +
        '<span class="rt">' + esc(t.origin || "") + ' → ' + esc(t.dest || "") + '</span>' +
        '<span class="pa">' + delayVal(t.predicted) + '<small>pred</small></span>' +
      '</div>'
    );
  }

  function settledCard(t) {
    var delta = t.delta != null ? t.delta : (t.actual != null && t.predicted != null ? t.actual - t.predicted : null);
    return (
      '<div class="mini" style="--c:' + esc(t.brand || "#94a3b8") + '">' +
        '<span class="hc">' + esc(t.label || "") + '</span>' +
        '<span class="rt">' + esc(t.origin || "") + ' → ' + esc(t.dest || "") + '</span>' +
        '<span class="pa">' +
          delayVal(t.predicted) + '→' + delayVal(t.actual) +
          '<small>' + esc(accTxt(delta)) + '</small>' +
        '</span>' +
      '</div>'
    );
  }

  /* ── App initialisers ─────────────────────────────────────────────── */

  window.OsApps = {

    /**
     * Fill [data-fill] nodes within root from window.MAP.
     * Called internally by other initialisers — also exposed so the shell
     * can call it directly if needed.
     */
    fill: function (root) {
      fill(root);
    },

    /* ── Operators ──────────────────────────────────────────────────── */
    operators: function (root) {
      fill(root);
      // Yesterday's per-operator on-time % + journey count, as a ranked bar league.
      renderOperatorLeague(root.querySelector("#op-list"), window.MAP_OPS_DAY || []);
    },

    /* ── Predictions ────────────────────────────────────────────────── */
    predictions: function (root) {
      fill(root);
      // "Forecasts scored" — the full-day count from the About headline.
      var A = window.MAP_ABOUT || {};
      var cEl = root.querySelector("#pred-count");
      if (cEl) cEl.textContent = A.predictions != null ? Number(A.predictions).toLocaleString() : "—";
      // Signed forecast-error histogram (actual − predicted) from the day's outcomes.
      renderHistogram(root.querySelector("#pred-histo"), window.MAP_REPLAY || []);
    },

    /* ── Reliability ────────────────────────────────────────────────── */
    reliability: function (root) {
      fill(root);
      var M = window.MAP || {};
      // Gauges: arrival punctuality (band-coloured so the fill itself signals
      // quality) + delay recovery (a "more is better" rate, neutral accent).
      var arr = M.arrival_on_time_pct;
      setGauge(root.querySelector("#rel-arr"), arr, pctBand(arr == null ? 0 : arr));
      setGauge(root.querySelector("#rel-rec"), M.recovered_pct, "#2563eb");
      // Hourly "when the network runs late" profile (% of trains 5+ min late) + peak.
      renderHourly(root.querySelector("#rel-hours"), root.querySelector("#rel-hours-axis"), root.querySelector("#rel-peak"), window.MAP_HOURLY || []);
      // Most reliable operators — top 8 by on-time %.
      renderOperatorLeague(root.querySelector("#rel-op-list"), M.operators || [], 8);
    },

    /* ── About ──────────────────────────────────────────────────────── */
    about: function (root) {
      fill(root);
      // Headline engine stats (yesterday) from window.MAP_ABOUT.
      var A = window.MAP_ABOUT || {};
      function setn(id, v) { var e = root.querySelector("#" + id); if (e) e.textContent = v; }
      function kfmt(v) { return v >= 10000 ? (Math.round(v / 100) / 10) + "k" : Number(v).toLocaleString(); }
      function fdate(s) {
        var p = String(s || "").split("-");
        if (p.length !== 3) return s || "—";
        var mo = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        return Number(p[2]) + " " + (mo[Number(p[1]) - 1] || "") + " " + p[0];
      }
      setn("ab-journeys", A.journeys ? Number(A.journeys).toLocaleString() : "—");
      setn("ab-calls", A.calls ? kfmt(A.calls) : "—");
      setn("ab-preds", A.predictions ? Number(A.predictions).toLocaleString() : "—");
      setn("ab-mae", A.mae != null ? A.mae + " min" : "—");
      setn("ab-within5", A.within5 != null ? A.within5 + "%" : "—");
      setn("ab-ontime", A.ontime != null ? A.ontime + "%" : "—");
      setn("ab-date", fdate(A.date));
    },

    /* ── Replay ─────────────────────────────────────────────────────── */
    replay: function (root) {
      fill(root);
      // Replay yesterday's full day of real forecasts: each service appears in
      // "Tracking" at its departure (showing the forecast), then moves to "Just
      // settled" with forecast → actual once it's run. Driven by a shared clock.
      var R = window.MAP_REPLAY || [];
      var clockEl = root.querySelector("#rp-frame");
      var trackEl = root.querySelector("#rp-track");
      var settledEl = root.querySelector("#rp-settled");
      if (!R.length) {
        if (trackEl) trackEl.innerHTML = '<p class="empty">No replay data.</p>';
        if (settledEl) settledEl.innerHTML = '<p class="empty">—</p>';
        return;
      }
      // Full-day accuracy scoreboard — computed once over every scored service,
      // so Replay shows the verdict the Map can't: how the forecasts actually did.
      var scoreEl = root.querySelector("#rp-score");
      if (scoreEl) {
        var spot = 0, w2 = 0, sum = 0, n = R.length, i0;
        for (i0 = 0; i0 < n; i0++) { var e0 = Math.abs(R[i0].a - R[i0].p); sum += e0; if (e0 === 0) spot++; if (e0 <= 2) w2++; }
        scoreEl.innerHTML =
          '<div class="rs"><b>' + Math.round(100 * spot / n) + '%</b><span>spot on</span></div>' +
          '<div class="rs"><b>' + Math.round(100 * w2 / n) + '%</b><span>within 2 min</span></div>' +
          '<div class="rs"><b>' + (Math.round(sum / n * 10) / 10) + ' min</b><span>average miss</span></div>' +
          '<div class="rs"><b>' + Number(n).toLocaleString() + '</b><span>services replayed</span></div>';
      }
      var RT = 210000, RUN = 30, SETTLE = 12; // ms/day, replay-min running / settled
      function pad(x) { return (x < 10 ? "0" : "") + x; }
      function hhmm(m) { m = ((m % 1440) + 1440) % 1440; return pad(Math.floor(m / 60)) + ":" + pad(Math.floor(m % 60)); }
      function dcol(v) { return v <= 1 ? "#34d399" : v <= 5 ? "#f2c14e" : "#f04545"; }
      // Tracking cards colour by the FORECAST (the actual isn't known yet); settled
      // cards colour by the actual outcome.
      function runCard(r) {
        return '<div class="mini" style="--c:' + dcol(r.p) + '"><span class="hc">' + esc(r.l) + '</span><span class="rt">' + esc(r.o) + ' → ' + esc(r.d) + '</span><span class="pa">' + delayVal(r.p) + '<small>forecast</small></span></div>';
      }
      function setCard(r) {
        return '<div class="mini" style="--c:' + dcol(r.a) + '"><span class="hc">' + esc(r.l) + '</span><span class="rt">' + esc(r.o) + ' → ' + esc(r.d) + '</span><span class="pa">' + delayVal(r.p) + '→' + delayVal(r.a) + '<small>' + esc(accTxt(r.a - r.p)) + '</small></span></div>';
      }
      // Shared replay epoch so this clock stays in lockstep with the Live Map's.
      if (window.__rpT0 == null) window.__rpT0 = (window.performance && performance.now) ? performance.now() : Date.now();
      function tick() {
        var now = (window.performance && performance.now) ? performance.now() : Date.now();
        var clock = (((now - window.__rpT0) % RT) / RT) * 1440;
        if (clockEl) clockEl.textContent = "Yesterday · " + hhmm(clock);
        var run = [], set = [];
        for (var i = 0; i < R.length; i++) {
          var r = R[i], end = r.t + RUN;
          if (clock >= r.t && clock < end) run.push(r);
          else if (clock >= end && clock < end + SETTLE) set.push(r);
        }
        run.sort(function (a, b) { return b.t - a.t; });
        if (trackEl) trackEl.innerHTML = run.slice(0, 9).map(runCard).join("") || '<p class="empty">No services running at this moment.</p>';
        if (settledEl) settledEl.innerHTML = set.slice(0, 7).map(setCard).join("") || '<p class="empty">Nothing settled in the last few minutes.</p>';
      }
      tick();
      var timer = setInterval(tick, 300);
      var winEl = root.closest ? root.closest(".win-body") : null;
      if (winEl) winEl._replayTimer = timer;
    }
  };

}());
