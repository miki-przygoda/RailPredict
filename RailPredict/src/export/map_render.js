// Exposed as RailPredictMap.init() so the standalone map page can call it on
// load and the OS shell can call it when the Map window opens.
window.RailPredictMap = { init: function () {
  "use strict";
  var M = window.MAP || {};
  var outline = window.GB_OUTLINE, rail = window.GB_RAIL;
  var svg = document.getElementById("map-svg");
  var NS = "http://www.w3.org/2000/svg";

  // --- fill [data-fill] KPI hooks ---
  function get(o, p) { return p.split(".").reduce(function (a, k) { return a == null ? null : a[k]; }, o); }
  function fmt(v, sfx) { if (v == null) return "—"; if (typeof v === "number") v = Math.round(v * 10) / 10; return sfx ? v + sfx : v; }
  document.querySelectorAll("[data-fill]").forEach(function (e) {
    e.textContent = fmt(get(M, e.getAttribute("data-fill")), e.getAttribute("data-suffix"));
  });

  // Cancel any prior animation loop UNCONDITIONALLY — even if we early-return
  // below, a stale loop must not keep mutating a now-detached SVG (the live
  // /map page re-inits on every poll).
  if (window.RailPredictMap._raf) cancelAnimationFrame(window.RailPredictMap._raf);

  if (!svg || !window.d3 || !outline) return;

  // Idempotent: clear the SVG so this can be safely re-called.
  while (svg.firstChild) svg.removeChild(svg.firstChild);

  var rect = svg.parentNode.getBoundingClientRect();
  var W = Math.max(Math.round(rect.width), 240), H = Math.max(Math.round(rect.height), 360);
  svg.setAttribute("viewBox", "0 0 " + W + " " + H);

  // Frame the camera on the GB mainland, NOT the full outline: fitting to the
  // whole dataset would zoom out to include far-flung outliers (Shetland ~60.8°N,
  // St Kilda ~-8.6°) and leave the mainland small and distant. We fit to a fixed
  // mainland box; all land is still drawn, but anything outside the frame falls
  // outside the SVG viewport and clips away.
  var GB_FRAME = { type: "Polygon", coordinates: [[[-6.4, 49.9], [1.9, 49.9], [1.9, 58.75], [-6.4, 58.75], [-6.4, 49.9]]] };
  var projection = d3.geoMercator().fitExtent([[10, 10], [W - 10, H - 10]], GB_FRAME);
  // Lock the view at ~25× (the slider mid-point) centred on the Manchester rail
  // hub, baked straight into the projection. There is deliberately NO interactive
  // zoom/transform layer: that layer scaled the glow filters every frame and
  // slowed the browser. A static baked zoom renders cheaply.
  projection.center([-2.2, 53.47]).translate([W / 2, H / 2]).scale(projection.scale() * 25);
  var path = d3.geoPath(projection);

  function el(tag, attrs) { var e = document.createElementNS(NS, tag); for (var k in attrs) e.setAttribute(k, attrs[k]); return e; }

  var gLand = el("g", {}); svg.appendChild(gLand);
  var gRail = el("g", {}); svg.appendChild(gRail);
  var gDots = el("g", {}); svg.appendChild(gDots);

  // Land fills the inland viewport (the coastline is off-screen at this zoom).
  (outline.features || [outline]).forEach(function (f) {
    gLand.appendChild(el("path", { d: path(f) || "", fill: "#22405f", stroke: "#7fb2e8", "stroke-width": "1.5", "stroke-linejoin": "round" }));
  });
  // Detailed OSM rail geometry — the real network is the basemap at this zoom.
  if (rail) (rail.features || [rail]).forEach(function (f) {
    gRail.appendChild(el("path", { d: path(f) || "", fill: "none", stroke: "#8fc4f5", "stroke-width": "1.2", "stroke-opacity": "0.55", "stroke-linejoin": "round" }));
  });

  function colorFor(v) { return v <= 0 ? "#34d399" : v <= 5 ? "#f2c14e" : "#f04545"; }
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "0m"; }
  function accTxt(d) { return d === 0 ? "spot on" : "off by " + d + "m"; }
  function trackCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '<small>pred</small></span></div>'; }
  function settledCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '→' + delayVal(t.actual) + '<small>' + accTxt(t.delta) + '</small></span></div>'; }

  // Trains are anchored at their ORIGIN station's real coordinates — not an
  // interpolated straight line — so every dot sits on the actual rail network.
  // Static (no animation loop), which keeps this deep-zoom view cheap to render;
  // only origins that fall inside the viewport are drawn.
  (M.trains || []).forEach(function (t) {
    var xy = t.o && projection(t.o);
    if (!xy || xy[0] < -30 || xy[0] > W + 30 || xy[1] < -30 || xy[1] > H + 30) return;
    var c = colorFor(t.predicted);
    var dot = el("circle", { cx: xy[0], cy: xy[1], r: "6", fill: c, "fill-opacity": "0.95", stroke: "#0a1422", "stroke-width": "1.5" });
    dot.style.filter = "drop-shadow(0 0 5px " + c + ")";
    gDots.appendChild(dot);
  });

  // Side banners, populated once: split by whether the outcome is known (settled)
  // or still a live prediction (tracking).
  var trackBox = document.getElementById("rail-track");
  var settleBox = document.getElementById("rail-settled");
  var trackList = [], settledList = [];
  (M.trains || []).forEach(function (t) {
    if (t.actual !== t.predicted || t.delta) settledList.push(t); else trackList.push(t);
  });
  if (trackBox) trackBox.innerHTML = trackList.slice(0, 6).map(trackCard).join("") || '<p class="empty">—</p>';
  if (settleBox) settleBox.innerHTML = settledList.slice(0, 6).map(settledCard).join("") || '<p class="empty">—</p>';
} };
