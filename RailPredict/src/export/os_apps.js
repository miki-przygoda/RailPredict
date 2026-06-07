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

  /**
   * Render an array of operator objects into a container element.
   * Each row: brand swatch · name · on_time_pct% · journey count.
   *
   * @param {Element} container  - DOM element to populate
   * @param {Array}   operators  - [{name, brand, on_time_pct, journeys}, …]
   * @param {number}  [maxRows]  - cap rows (default unlimited)
   */
  function renderOperatorList(container, operators, maxRows) {
    if (!container) return;
    if (!operators || operators.length === 0) {
      container.innerHTML = '<p class="empty">No operator data available.</p>';
      return;
    }
    var rows = maxRows ? operators.slice(0, maxRows) : operators;
    var html = "";
    for (var i = 0; i < rows.length; i++) {
      var op = rows[i];
      var brand = esc(op.brand || "#94a3b8");
      var name = esc(op.name || "Unknown");
      var pct = op.on_time_pct != null ? Math.round(op.on_time_pct * 10) / 10 + "%" : "—";
      var journeys = op.journeys != null ? Number(op.journeys).toLocaleString() : "—";
      html +=
        '<div class="op-row">' +
          '<span class="op-swatch" style="background:' + brand + '"></span>' +
          '<span class="op-name">' + name + '</span>' +
          '<span class="op-pct">' + esc(pct) + '</span>' +
          '<span class="op-journeys">' + esc(journeys) + ' journeys</span>' +
        '</div>';
    }
    container.innerHTML = html;
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
      // Yesterday's per-operator on-time % and journey count; swatch by performance.
      var ops = (window.MAP_OPS_DAY || []).map(function (o) {
        var c = o.otp >= 90 ? "#34d399" : o.otp >= 80 ? "#f2c14e" : "#f04545";
        return { name: o.name, on_time_pct: o.otp, journeys: o.j, brand: c };
      });
      renderOperatorList(root.querySelector("#op-list"), ops);
    },

    /* ── Predictions ────────────────────────────────────────────────── */
    predictions: function (root) {
      fill(root);
      // Static explanatory content is already in the template; nothing more needed.
    },

    /* ── Reliability ────────────────────────────────────────────────── */
    reliability: function (root) {
      fill(root);
      var M = window.MAP || {};
      var container = root.querySelector("#rel-op-list");
      // Show top 8 operators sorted by on_time_pct (already sorted by generator,
      // but guard in case the Rust task sorts by journeys instead).
      var ops = (M.operators || []).slice().sort(function (a, b) {
        return (b.on_time_pct || 0) - (a.on_time_pct || 0);
      });
      renderOperatorList(container, ops, 8);
    },

    /* ── About ──────────────────────────────────────────────────────── */
    about: function (root) {
      fill(root);
      // Static narrative is fully in the template; fill() handles generated_at.
    },

    /* ── Replay ─────────────────────────────────────────────────────── */
    replay: function (root) {
      fill(root);
      // Replay yesterday's full day of real predictions: each service appears in
      // "Tracking" at its departure (showing the predicted delay), then moves to
      // "Just settled" with predicted → actual once it's run. Driven by a clock.
      var R = window.MAP_REPLAY || [];
      var clockEl = root.querySelector("#rp-frame");
      var trackEl = root.querySelector("#rp-track");
      var settledEl = root.querySelector("#rp-settled");
      if (!R.length) {
        if (trackEl) trackEl.innerHTML = '<p class="empty">No replay data.</p>';
        if (settledEl) settledEl.innerHTML = '<p class="empty">—</p>';
        return;
      }
      var RT = 210000, RUN = 30, SETTLE = 12; // ms/day, replay-min running / settled
      function pad(n) { return (n < 10 ? "0" : "") + n; }
      function hhmm(m) { m = ((m % 1440) + 1440) % 1440; return pad(Math.floor(m / 60)) + ":" + pad(Math.floor(m % 60)); }
      function dcol(v) { return v <= 1 ? "#34d399" : v <= 5 ? "#f2c14e" : "#f04545"; }
      function runCard(r) {
        return '<div class="mini" style="--c:' + dcol(r.a) + '"><span class="hc">' + esc(r.l) + '</span><span class="rt">' + esc(r.o) + ' → ' + esc(r.d) + '</span><span class="pa">' + delayVal(r.p) + '<small>pred</small></span></div>';
      }
      function setCard(r) {
        return '<div class="mini" style="--c:' + dcol(r.a) + '"><span class="hc">' + esc(r.l) + '</span><span class="rt">' + esc(r.o) + ' → ' + esc(r.d) + '</span><span class="pa">' + delayVal(r.p) + '→' + delayVal(r.a) + '<small>' + esc(accTxt(r.a - r.p)) + '</small></span></div>';
      }
      var t0 = (window.performance && performance.now) ? performance.now() : Date.now();
      function tick() {
        var now = (window.performance && performance.now) ? performance.now() : Date.now();
        var clock = (((now - t0) % RT) / RT) * 1440;
        if (clockEl) clockEl.textContent = hhmm(clock);
        var run = [], set = [];
        for (var i = 0; i < R.length; i++) {
          var r = R[i], end = r.t + RUN;
          if (clock >= r.t && clock < end) run.push(r);
          else if (clock >= end && clock < end + SETTLE) set.push(r);
        }
        run.sort(function (a, b) { return b.t - a.t; });
        if (trackEl) trackEl.innerHTML = run.slice(0, 9).map(runCard).join("") || '<p class="empty">—</p>';
        if (settledEl) settledEl.innerHTML = set.slice(0, 7).map(setCard).join("") || '<p class="empty">—</p>';
      }
      tick();
      var timer = setInterval(tick, 300);
      var winEl = root.closest ? root.closest(".win-body") : null;
      if (winEl) winEl._replayTimer = timer;
    }
  };

}());
