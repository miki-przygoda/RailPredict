(function () {
  var D = window.DEMO || {};
  var esc = function (s) {
    return String(s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  };

  // --- fill [data-fill] hooks (supports dotted paths + optional suffix) ---
  function get(obj, path) {
    return path.split(".").reduce(function (o, k) { return o == null ? null : o[k]; }, obj);
  }
  function fmt(v, suffix) {
    if (v == null) return "—";
    if (typeof v === "number") v = Math.round(v * 10) / 10;
    return suffix ? v + suffix : v;
  }
  document.querySelectorAll("[data-fill]").forEach(function (el) {
    el.textContent = fmt(get(D, el.getAttribute("data-fill")), el.getAttribute("data-suffix"));
  });

  // --- operator highlights ---
  var opList = document.getElementById("op-list");
  if (opList && D.operators) {
    opList.innerHTML = D.operators.map(function (o) {
      return '<div class="op" style="--op:' + esc(o.brand) + '">' +
        '<span class="bar"></span><span class="nm">' + esc(o.name) + '</span>' +
        '<span class="pct">' + Math.round(o.on_time_pct) + '%</span>' +
        '<span class="ct">' + o.journeys + ' jrn</span></div>';
    }).join("");
  }

  // --- replay board renderer (mirrors static/board.js markup) ---
  function predChip(p) {
    if (p <= 0) return '<span class="pchip p-ontime">on time</span>';
    return '<span class="pchip ' + (p <= 5 ? "p-min" : "p-late") + '">+' + p + 'm</span>';
  }
  function delayText(v, pre) { return v <= 0 ? pre + " on time" : pre + " +" + v + "m"; }
  function accChip(d) {
    var cls = d <= 3 ? "acc-good" : d <= 8 ? "acc-ok" : "acc-bad";
    return '<span class="acc ' + cls + '">Δ' + d + '</span>';
  }
  function trackCard(t) {
    return '<div class="tcard" data-rid="' + esc(t.rid) + '" style="--op:' + esc(t.brand) + '">' +
      '<div class="tc-top"><span class="tc-hc">' + esc(t.label) + '</span><span class="tc-op">' + esc(t.operator) + '</span></div>' +
      '<div class="tc-route"><code>' + esc(t.origin) + '</code><span class="ar">→</span><code>' + esc(t.dest) + '</code></div>' +
      '<div class="tc-bot"><span class="tc-sched">dep ' + esc(t.scheduled) + '</span>' + predChip(t.predicted) + '</div></div>';
  }
  function settledCard(s) {
    return '<div class="tcard" data-rid="' + esc(s.rid) + '" style="--op:' + esc(s.brand) + '">' +
      '<div class="tc-top"><span class="tc-hc">' + esc(s.label) + '</span><span class="tc-op">' + esc(s.operator) + '</span></div>' +
      '<div class="tc-route"><code>' + esc(s.origin) + '</code><span class="ar">→</span><code>' + esc(s.dest) + '</code></div>' +
      '<div class="tc-bot settled-row"><span class="pa pred">' + delayText(s.predicted, "pred") + '</span><span class="ar">→</span><span class="pa act">' + delayText(s.actual, "actual") + '</span>' + accChip(s.delta) + '</div></div>';
  }
  var board = document.getElementById("replay-board");
  var frameLabel = document.getElementById("replay-frame");
  function renderFrame(snap, existing) {
    var tracking = (snap.tracking || []).map(trackCard).join("");
    var settled = (snap.settled || []).map(settledCard).join("");
    board.innerHTML =
      '<div><div class="zone-head"><h3>Tracking now</h3><span class="ct">' + (snap.tracking || []).length + ' trains</span></div><div class="tgrid">' + (tracking || '<p class="panel-empty">—</p>') + '</div></div>' +
      '<div><div class="zone-head"><h3>Just settled</h3><span class="ct">predicted vs actual</span></div><div class="tgrid">' + (settled || '<p class="panel-empty">—</p>') + '</div></div>';
    board.querySelectorAll("[data-rid]").forEach(function (el) {
      if (existing && !existing.has(el.dataset.rid)) el.classList.add("fresh");
    });
  }

  // --- replay clock: auto-starts when the replay beat scrolls into view ---
  var frames = D.frames || [];
  var idx = 0, timer = null, started = false;
  function tick() {
    if (!frames.length) return;
    var existing = new Set([].map.call(board.querySelectorAll("[data-rid]"), function (e) { return e.dataset.rid; }));
    renderFrame(frames[idx], existing);
    if (frameLabel) frameLabel.textContent = (idx + 1) + " / " + frames.length;
    idx = (idx + 1) % frames.length; // loop so the demo never dead-ends
  }
  function startReplay() {
    if (started || !frames.length) return;
    started = true;
    tick();
    timer = setInterval(tick, 2600);
  }

  // --- scroll reveal + replay trigger ---
  if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) {
          e.target.classList.add("in");
          if (e.target.id === "beat-replay") startReplay();
        }
      });
    }, { threshold: 0.18 });
    document.querySelectorAll(".reveal").forEach(function (s) { io.observe(s); });
  } else {
    document.querySelectorAll(".reveal").forEach(function (s) { s.classList.add("in"); });
    startReplay();
  }
})();
