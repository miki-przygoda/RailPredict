// Exposed as RailPredictMap.init() so the standalone map page can call it on
// load and the OS shell can call it when the Map window opens.
//
// Basemap is a real OSM/CARTO map snapshot (a raster baked into the page) with
// known Web-Mercator bounds. Train dots are placed by exact lat/lon using the
// same slippy-tile maths the snapshot was stitched with, so they line up to the
// pixel. No projection library, no vector geometry — and nothing animates, so a
// deep-zoom static view stays cheap.
window.RailPredictMap = { init: function () {
  "use strict";
  var M = window.MAP || {};
  var B = window.MAP_BOUNDS;   // { z, x0, y0, w, h } — the snapshot's tile origin + pixel size
  var IMG = window.MAP_IMG;    // basemap as a data: URL
  var svg = document.getElementById("map-svg");
  var NS = "http://www.w3.org/2000/svg";
  var XLINK = "http://www.w3.org/1999/xlink";

  // --- fill [data-fill] KPI hooks ---
  function get(o, p) { return p.split(".").reduce(function (a, k) { return a == null ? null : a[k]; }, o); }
  function fmt(v, sfx) { if (v == null) return "—"; if (typeof v === "number") v = Math.round(v * 10) / 10; return sfx ? v + sfx : v; }
  document.querySelectorAll("[data-fill]").forEach(function (e) {
    e.textContent = fmt(get(M, e.getAttribute("data-fill")), e.getAttribute("data-suffix"));
  });

  if (!svg || !B || !IMG) return;

  // Idempotent: clear the SVG so this can be safely re-called (the live /map page
  // re-inits on each poll).
  while (svg.firstChild) svg.removeChild(svg.firstChild);

  // The SVG coordinate system IS the snapshot's pixel grid; it scales to the
  // container (letterboxed) so the basemap and the dots always move together.
  svg.setAttribute("viewBox", "0 0 " + B.w + " " + B.h);
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  function el(tag, attrs) { var e = document.createElementNS(NS, tag); for (var k in attrs) e.setAttribute(k, attrs[k]); return e; }

  // Basemap raster.
  var img = el("image", { x: "0", y: "0", width: B.w, height: B.h });
  img.setAttributeNS(XLINK, "href", IMG);
  img.setAttribute("href", IMG);
  svg.appendChild(img);

  // lon/lat -> snapshot pixel (Web-Mercator, identical maths to the tile stitch).
  var N = Math.pow(2, B.z);
  function lon2px(lon) { return ((lon + 180) / 360 * N - B.x0) * 256; }
  function lat2px(lat) { var r = lat * Math.PI / 180; return ((1 - Math.asinh(Math.tan(r)) / Math.PI) / 2 * N - B.y0) * 256; }

  function colorFor(v) { return v <= 0 ? "#34d399" : v <= 5 ? "#f2c14e" : "#f04545"; }
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "0m"; }
  function accTxt(d) { return d === 0 ? "spot on" : "off by " + d + "m"; }
  function trackCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '<small>pred</small></span></div>'; }
  function settledCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '→' + delayVal(t.actual) + '<small>' + accTxt(t.delta) + '</small></span></div>'; }

  // Station network — a dim node at every known station, drawn under the trains.
  var stations = window.MAP_STATIONS || [];
  if (stations.length) {
    var gSt = el("g", {});
    for (var si = 0; si < stations.length; si++) {
      var sx = lon2px(stations[si][0]), sy = lat2px(stations[si][1]);
      if (sx < -10 || sx > B.w + 10 || sy < -10 || sy > B.h + 10) continue;
      gSt.appendChild(el("circle", { cx: sx.toFixed(1), cy: sy.toFixed(1), r: "4", fill: "#7fa8d8", "fill-opacity": "0.5" }));
    }
    svg.appendChild(gSt);  // append once, after building, to avoid per-node reflow
  }

  // Trains plotted at their ORIGIN station's real coordinates, coloured by
  // predicted delay. Static — no animation loop.
  var gDots = el("g", {}); svg.appendChild(gDots);
  (M.trains || []).forEach(function (t) {
    if (!t.o) return;
    var x = lon2px(t.o[0]), y = lat2px(t.o[1]);
    if (x < -20 || x > B.w + 20 || y < -20 || y > B.h + 20) return;
    var c = colorFor(t.predicted);
    var dot = el("circle", { cx: x, cy: y, r: "12", fill: c, "fill-opacity": "0.95", stroke: "#06101e", "stroke-width": "3" });
    dot.style.filter = "drop-shadow(0 0 8px " + c + ")";
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
