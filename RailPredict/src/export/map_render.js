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
  // Lock the view at ~25× (the slider mid-point) centred on the dense N-England
  // core, baked straight into the projection. There is deliberately NO interactive
  // zoom/transform layer: that layer scaled the dots' glow filters every animation
  // frame, which slowed the browser. A static baked zoom renders cheaply.
  projection.center([-1.9, 53.5]).translate([W / 2, H / 2]).scale(projection.scale() * 25);
  var path = d3.geoPath(projection);

  function el(tag, attrs) { var e = document.createElementNS(NS, tag); for (var k in attrs) e.setAttribute(k, attrs[k]); return e; }

  var gLand = el("g", {}); svg.appendChild(gLand);
  var gRail = el("g", {}); svg.appendChild(gRail);
  var gDots = el("g", {}); svg.appendChild(gDots);

  (outline.features || [outline]).forEach(function (f) {
    gLand.appendChild(el("path", { d: path(f) || "", fill: "#27466e", stroke: "#7fb2e8", "stroke-width": "1.5", "stroke-linejoin": "round", "stroke-linecap": "round" }));
  });
  if (rail) (rail.features || [rail]).forEach(function (f) {
    gRail.appendChild(el("path", { d: path(f) || "", fill: "none", stroke: "#9ecbf2", "stroke-width": "0.65", "stroke-opacity": "0.4" }));
  });

  function colorFor(v) { return v <= 0 ? "#34d399" : v <= 5 ? "#f2c14e" : "#f04545"; }
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "0m"; }
  function accTxt(d) { return d === 0 ? "spot on" : "off by " + d + "m"; }
  function trackCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '<small>pred</small></span></div>'; }
  function settledCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '→' + delayVal(t.actual) + '<small>' + accTxt(t.delta) + '</small></span></div>'; }

  var trains = (M.trains || []).map(function (t, i) {
    var dot = el("circle", { r: "3.4", fill: colorFor(t.predicted), "fill-opacity": "0.95" });
    dot.style.filter = "drop-shadow(0 0 4px " + colorFor(t.predicted) + ")";
    gDots.appendChild(dot);
    return { t: t, dot: dot, interp: d3.geoInterpolate(t.o, t.d), dur: 9000 + (i % 7) * 900, phase: (i * 1373) % 11000 };
  });

  var trackBox = document.getElementById("rail-track");
  var settleBox = document.getElementById("rail-settled");
  var lastBanner = 0;

  function frame(now) {
    var settledList = [], trackList = [];
    for (var i = 0; i < trains.length; i++) {
      var a = trains[i];
      var p = ((now + a.phase) % a.dur) / a.dur;
      var arriving = p > 0.88;
      // Banners list all active trains regardless of where they sit on the map.
      if (arriving) settledList.push(a.t); else if (p > 0.4) trackList.push(a.t);
      // Only render dots inside the viewport — at this zoom most trains are
      // off-screen, and rendering their glow filters would burn frame time.
      var ll = a.interp(p), xy = ll && projection(ll);
      if (xy && xy[0] > -20 && xy[0] < W + 20 && xy[1] > -20 && xy[1] < H + 20) {
        a.dot.setAttribute("cx", xy[0]);
        a.dot.setAttribute("cy", xy[1]);
        a.dot.setAttribute("fill", colorFor(arriving ? a.t.actual : a.t.predicted));
        a.dot.setAttribute("r", arriving ? "4.8" : "3.4");
        a.dot.setAttribute("display", "");
      } else {
        a.dot.setAttribute("display", "none");
      }
    }
    if (now - lastBanner > 350) {
      lastBanner = now;
      if (trackBox) trackBox.innerHTML = trackList.slice(0, 6).map(trackCard).join("") || '<p class="empty">—</p>';
      if (settleBox) settleBox.innerHTML = settledList.slice(0, 4).map(settledCard).join("") || '<p class="empty">—</p>';
    }
    window.RailPredictMap._raf = requestAnimationFrame(frame);
  }
  window.RailPredictMap._raf = requestAnimationFrame(frame);
} };
