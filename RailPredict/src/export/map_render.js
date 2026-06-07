// Exposed as RailPredictMap.init() so the standalone map page can call it on
// load and the OS shell can call it when the Map window opens.
//
// Basemap is a real OSM/CARTO map snapshot (raster baked into the page) with
// known Web-Mercator bounds. On top we light up the rail network: every segment
// observed in service, coloured by its average delay (green/amber/red), with
// white pulses flowing along the busiest corridors. lat/lon -> pixel uses the
// same slippy-tile maths the snapshot was stitched with, so everything aligns to
// the pixel. Scroll to zoom, drag to pan; a fullscreen button expands the map.
window.RailPredictMap = { init: function () {
  "use strict";
  var M = window.MAP || {};
  var B = window.MAP_BOUNDS;          // { z, ox, oy, w, h } — snapshot origin (world px) + size
  var IMG = window.MAP_IMG;           // basemap data: URL
  var stations = window.MAP_STATIONS || [];  // [[lon,lat],...]
  var edges = window.MAP_EDGES || []; // [[i,j,bucket,count],...]
  var svg = document.getElementById("map-svg");
  var NS = "http://www.w3.org/2000/svg";
  var XLINK = "http://www.w3.org/1999/xlink";
  var RP = window.RailPredictMap;

  // --- fill [data-fill] KPI hooks ---
  function get(o, p) { return p.split(".").reduce(function (a, k) { return a == null ? null : a[k]; }, o); }
  function fmt(v, sfx) { if (v == null) return "—"; if (typeof v === "number") v = Math.round(v * 10) / 10; return sfx ? v + sfx : v; }
  document.querySelectorAll("[data-fill]").forEach(function (e) {
    e.textContent = fmt(get(M, e.getAttribute("data-fill")), e.getAttribute("data-suffix"));
  });

  // Tear down anything from a previous init (the live /map page re-inits each poll).
  if (RP._raf) cancelAnimationFrame(RP._raf);
  if (RP._ac) RP._ac.abort();
  var ac = new AbortController(); RP._ac = ac; var sig = ac.signal;

  if (!svg || !B || !IMG) return;
  while (svg.firstChild) svg.removeChild(svg.firstChild);

  svg.setAttribute("viewBox", "0 0 " + B.w + " " + B.h);
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  function el(tag, attrs) { var e = document.createElementNS(NS, tag); for (var k in attrs) e.setAttribute(k, attrs[k]); return e; }

  // Everything that pans/zooms together lives under gZoom.
  var gZoom = el("g", {}); svg.appendChild(gZoom);

  // Basemap (dimmed so the network glows on top).
  var img = el("image", { x: "0", y: "0", width: B.w, height: B.h, opacity: "0.78" });
  img.setAttributeNS(XLINK, "href", IMG);
  img.setAttribute("href", IMG);
  gZoom.appendChild(img);

  // lon/lat -> snapshot pixel.
  var N = Math.pow(2, B.z) * 256;
  function lon2px(lon) { return (lon + 180) / 360 * N - B.ox; }
  function lat2px(lat) { var r = lat * Math.PI / 180; return (1 - Math.asinh(Math.tan(r)) / Math.PI) / 2 * N - B.oy; }

  // --- Lit network: one path per delay bucket, glow (wide faint) then crisp. ---
  var COL = ["#34d399", "#f2c14e", "#f04545"]; // 0 on-time, 1 slight, 2 late
  var dp = ["", "", ""];
  for (var ei = 0; ei < edges.length; ei++) {
    var e = edges[ei], s1 = stations[e[0]], s2 = stations[e[1]];
    if (!s1 || !s2) continue;
    dp[e[2]] += "M" + lon2px(s1[0]).toFixed(1) + " " + lat2px(s1[1]).toFixed(1) +
                "L" + lon2px(s2[0]).toFixed(1) + " " + lat2px(s2[1]).toFixed(1);
  }
  // non-scaling-stroke keeps line weight constant at any zoom.
  [0, 1, 2].forEach(function (b) {
    gZoom.appendChild(el("path", { d: dp[b], stroke: COL[b], "stroke-width": "5", "stroke-opacity": "0.16", fill: "none", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke" }));
  });
  [0, 1, 2].forEach(function (b) {
    gZoom.appendChild(el("path", { d: dp[b], stroke: COL[b], "stroke-width": b === 2 ? "2" : "1.5", "stroke-opacity": b === 0 ? "0.65" : "0.9", fill: "none", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke" }));
  });

  // --- Pulses flowing along the busiest corridors ---
  var pe = edges.slice().sort(function (a, b) { return b[3] - a[3]; }).slice(0, 90);
  var pulses = [];
  for (var pi = 0; pi < pe.length; pi++) {
    var pe1 = stations[pe[pi][0]], pe2 = stations[pe[pi][1]];
    if (!pe1 || !pe2) continue;
    var ax = lon2px(pe1[0]), ay = lat2px(pe1[1]), bx = lon2px(pe2[0]), by = lat2px(pe2[1]);
    if (pi % 2) { var tx = ax; ax = bx; bx = tx; var ty = ay; ay = by; by = ty; } // alternate flow direction
    var dot = el("circle", { fill: "#eaf3ff", "fill-opacity": "0.95" });
    gZoom.appendChild(dot);
    pulses.push({ ax: ax, ay: ay, bx: bx, by: by, dot: dot, dur: 3200 + (pi % 9) * 420, phase: (pi * 617) % 4000 });
  }

  // --- Pan / zoom state (persisted across re-inits) ---
  var view = RP._view || { k: 1, tx: 0, ty: 0 };
  function clampPan() {
    var minX = B.w * (1 - view.k), minY = B.h * (1 - view.k);
    view.tx = Math.max(minX, Math.min(0, view.tx));
    view.ty = Math.max(minY, Math.min(0, view.ty));
  }
  function applyT() { clampPan(); gZoom.setAttribute("transform", "translate(" + view.tx.toFixed(2) + "," + view.ty.toFixed(2) + ") scale(" + view.k.toFixed(4) + ")"); RP._view = view; }
  function toVB(cx, cy) { var pt = svg.createSVGPoint(); pt.x = cx; pt.y = cy; var m = svg.getScreenCTM(); if (!m) return [0, 0]; var p = pt.matrixTransform(m.inverse()); return [p.x, p.y]; }
  applyT();

  svg.addEventListener("wheel", function (ev) {
    ev.preventDefault();
    var v = toVB(ev.clientX, ev.clientY);
    var lx = (v[0] - view.tx) / view.k, ly = (v[1] - view.ty) / view.k;
    var f = ev.deltaY < 0 ? 1.16 : 1 / 1.16;
    view.k = Math.max(1, Math.min(9, view.k * f));
    view.tx = v[0] - view.k * lx; view.ty = v[1] - view.k * ly;
    applyT();
  }, { passive: false, signal: sig });

  var drag = null;
  svg.addEventListener("mousedown", function (ev) { drag = toVB(ev.clientX, ev.clientY); svg.style.cursor = "grabbing"; ev.preventDefault(); }, { signal: sig });
  window.addEventListener("mousemove", function (ev) {
    if (!drag) return;
    var v = toVB(ev.clientX, ev.clientY);
    view.tx += v[0] - drag[0]; view.ty += v[1] - drag[1]; drag = v; applyT();
  }, { signal: sig });
  window.addEventListener("mouseup", function () { drag = null; svg.style.cursor = ""; }, { signal: sig });

  // Fullscreen toggle (the button lives in the .cc-map next to the svg).
  var fsBtn = document.getElementById("map-fs");
  var fsTarget = svg.parentNode;
  if (fsBtn && fsTarget) {
    fsBtn.onclick = function () {
      if (document.fullscreenElement) { if (document.exitFullscreen) document.exitFullscreen(); }
      else if (fsTarget.requestFullscreen) { fsTarget.requestFullscreen(); }
    };
  }

  // --- Side banners, once ---
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "0m"; }
  function accTxt(d) { return d === 0 ? "spot on" : "off by " + d + "m"; }
  function trackCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '<small>pred</small></span></div>'; }
  function settledCard(t) { return '<div class="mini" style="--c:' + esc(t.brand) + '"><span class="hc">' + esc(t.label) + '</span><span class="rt">' + esc(t.origin) + ' → ' + esc(t.dest) + '</span><span class="pa">' + delayVal(t.predicted) + '→' + delayVal(t.actual) + '<small>' + accTxt(t.delta) + '</small></span></div>'; }
  var trackBox = document.getElementById("rail-track");
  var settleBox = document.getElementById("rail-settled");
  var trackList = [], settledList = [];
  (M.trains || []).forEach(function (t) { if (t.actual !== t.predicted || t.delta) settledList.push(t); else trackList.push(t); });
  if (trackBox) trackBox.innerHTML = trackList.slice(0, 6).map(trackCard).join("") || '<p class="empty">—</p>';
  if (settleBox) settleBox.innerHTML = settledList.slice(0, 6).map(settledCard).join("") || '<p class="empty">—</p>';

  // --- Pulse animation (counter-scaled so dot size stays constant under zoom) ---
  function frame(now) {
    var r = (6 / view.k).toFixed(2);
    for (var i = 0; i < pulses.length; i++) {
      var p = pulses[i];
      var f = ((now + p.phase) % p.dur) / p.dur;
      p.dot.setAttribute("cx", (p.ax + (p.bx - p.ax) * f).toFixed(1));
      p.dot.setAttribute("cy", (p.ay + (p.by - p.ay) * f).toFixed(1));
      p.dot.setAttribute("r", r);
    }
    RP._raf = requestAnimationFrame(frame);
  }
  RP._raf = requestAnimationFrame(frame);
} };
