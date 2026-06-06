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
      var M = window.MAP || {};
      var container = root.querySelector("#op-list");
      renderOperatorList(container, M.operators);
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
      var M = window.MAP || {};
      var frames = M.frames || [];
      var frameEl = root.querySelector("#rp-frame");
      var trackEl = root.querySelector("#rp-track");
      var settledEl = root.querySelector("#rp-settled");

      if (frames.length === 0) {
        if (trackEl)   trackEl.innerHTML   = '<p class="empty">No frames available.</p>';
        if (settledEl) settledEl.innerHTML = '<p class="empty">—</p>';
        if (frameEl)   frameEl.textContent = "0 / 0";
        return;
      }

      var current = 0;
      var INTERVAL_MS = 2600;

      function renderFrame(idx) {
        var f = frames[idx] || {};
        var tracking = f.tracking || [];
        var settled  = f.settled  || [];

        if (frameEl) {
          frameEl.textContent = (idx + 1) + " / " + frames.length;
        }

        if (trackEl) {
          trackEl.innerHTML = tracking.length
            ? tracking.slice(0, 8).map(trackCard).join("")
            : '<p class="empty">—</p>';
        }

        if (settledEl) {
          settledEl.innerHTML = settled.length
            ? settled.slice(0, 6).map(settledCard).join("")
            : '<p class="empty">—</p>';
        }
      }

      // Initial render
      renderFrame(current);

      // Advance frame on interval.
      // Store the interval ID on the root element so closeWin doesn't leak it;
      // the shell already removes the DOM node, which stops any further renders.
      var timer = setInterval(function () {
        current = (current + 1) % frames.length;
        renderFrame(current);
      }, INTERVAL_MS);

      // Attach cleanup to the closest .win ancestor if possible, so the
      // interval is cleared when the window's DOM node is removed.
      // (Browsers GC it anyway once the node is detached, but this is clean.)
      var winEl = root.closest ? root.closest(".win-body") : null;
      if (winEl) {
        winEl._replayTimer = timer;
      }
    }
  };

}());
