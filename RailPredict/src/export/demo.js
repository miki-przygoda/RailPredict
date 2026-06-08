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

  // --- replay board renderer (banner cards; explicit predicted/actual/error) ---
  // A signed minute value: "+4m" late, "0m" on time, "-2m" early.
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "0m"; }
  // The prediction pill on a still-tracking train.
  function predChip(p) {
    if (p === 0) return '<span class="pchip p-ontime">on time</span>';
    if (p < 0) return '<span class="pchip p-ontime">' + (-p) + 'm early</span>';
    return '<span class="pchip ' + (p <= 5 ? "p-min" : "p-late") + '">+' + p + 'm</span>';
  }
  // How far the prediction was off — the accuracy of the call.
  function accChip(d) {
    var cls = d <= 3 ? "acc-good" : d <= 8 ? "acc-ok" : "acc-bad";
    var txt = d === 0 ? "spot on" : "off by " + d + "m";
    return '<span class="acc ' + cls + '">' + txt + '</span>';
  }
  function idBlock(c) {
    return '<div class="tc-id"><span class="tc-hc">' + esc(c.label) + '</span><span class="tc-op">' + esc(c.operator) + '</span></div>';
  }
  // Route is rendered last so the grid pins it to the right edge of every card.
  function routeBlock(c) {
    return '<div class="tc-route"><code>' + esc(c.origin) + '</code><span class="ar">→</span><code>' + esc(c.dest) + '</code></div>';
  }
  function trackCard(t) {
    return '<div class="tcard" data-rid="' + esc(t.rid) + '" style="--op:' + esc(t.brand) + '">' + idBlock(t) +
      '<div class="tc-meta">' +
        '<span class="tc-sched">dep ' + esc(t.scheduled) + '</span>' +
        '<div class="cell"><span class="k">Predicted</span>' + predChip(t.predicted) + '</div>' +
      '</div>' + routeBlock(t) + '</div>';
  }
  function settledCard(s) {
    return '<div class="tcard" data-rid="' + esc(s.rid) + '" style="--op:' + esc(s.brand) + '">' + idBlock(s) +
      '<div class="tc-meta">' +
        '<div class="cell"><span class="k">Predicted</span><span class="v pred">' + delayVal(s.predicted) + '</span></div>' +
        '<span class="cell arrow">→</span>' +
        '<div class="cell"><span class="k">Actual</span><span class="v act">' + delayVal(s.actual) + '</span></div>' +
        accChip(s.delta) +
      '</div>' + routeBlock(s) + '</div>';
  }
  var board = document.getElementById("replay-board");
  var frameLabel = document.getElementById("replay-frame");
  function renderFrame(snap, existing) {
    var tracking = (snap.tracking || []).map(trackCard).join("");
    var settled = (snap.settled || []).map(settledCard).join("");
    var nTrack = (snap.tracking || []).length;
    board.innerHTML =
      '<div class="zone zone-track"><div class="zone-head"><h3>Tracking now</h3><span class="ct">' + nTrack + ' live · predicted delay</span></div><div class="tgrid">' + (tracking || '<p class="panel-empty">—</p>') + '</div></div>' +
      '<div class="zone zone-settled"><div class="zone-head"><h3>Just settled</h3><span class="ct">predicted → actual</span></div><div class="tgrid">' + (settled || '<p class="panel-empty">—</p>') + '</div></div>';
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
