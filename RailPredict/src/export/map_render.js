// Exposed as RailPredictMap.init() so the standalone map page can call it on
// load and the OS shell can call it when the Map window opens.
//
// Basemap is a real OSM/CARTO snapshot (raster baked in) with known Web-Mercator
// bounds. On top: the rail network drawn as light grey lines (brighter where more
// trains run it, each link drawn once so nothing overlaps), and real multi-stop
// journeys playing out — each is a single node travelling its route, coloured by
// delay, appearing in "Tracking" while running and "Just settled" on arrival.
// lat/lon -> pixel uses the snapshot's slippy-tile maths so it all lines up.
// Scroll to zoom, drag to pan, ⛶ for fullscreen.
window.RailPredictMap = { init: function () {
  "use strict";
  var M = window.MAP || {};
  var B = window.MAP_BOUNDS;          // { z, ox, oy, w, h }
  var IMG = window.MAP_IMG;
  var stations = window.MAP_STATIONS || [];   // [[lon,lat],...]
  var edges = window.MAP_EDGES || [];          // [[i,j,bucket,count],...]
  var journeys = window.MAP_JOURNEYS || [];    // [{p:[idx...],lbl,o,d,dly,b},...]
  var svg = document.getElementById("map-svg");
  var NS = "http://www.w3.org/2000/svg";
  var XLINK = "http://www.w3.org/1999/xlink";
  var RP = window.RailPredictMap;
  var COL = ["#34d399", "#f2c14e", "#f04545"];   // delay band: on-time / slight / late
  var BRIGHT = ["#7dffcb", "#ffdd86", "#ff9a9a"]; // bright node cores per delay band

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
  var gZoom = el("g", {}); svg.appendChild(gZoom);

  var img = el("image", { x: "0", y: "0", width: B.w, height: B.h, opacity: "0.7" });
  img.setAttributeNS(XLINK, "href", IMG);
  img.setAttribute("href", IMG);
  gZoom.appendChild(img);

  var N = Math.pow(2, B.z) * 256;
  function lon2px(lon) { return (lon + 180) / 360 * N - B.ox; }
  function lat2px(lat) { var r = lat * Math.PI / 180; return (1 - Math.asinh(Math.tan(r)) / Math.PI) / 2 * N - B.oy; }

  // --- Base network: light grey lines, brighter where more trains run.
  // Each link is drawn once (deduped by station pair), so they don't stack. ---
  var GREY = [{ c: "#5b657c", o: "0.32", w: "1" }, { c: "#7886a0", o: "0.48", w: "1.15" }, { c: "#9cb1d0", o: "0.66", w: "1.35" }];
  var gp = ["", "", ""];
  for (var ei = 0; ei < edges.length; ei++) {
    var e = edges[ei], s1 = stations[e[0]], s2 = stations[e[1]];
    if (!s1 || !s2) continue;
    var tier = e[3] < 5 ? 0 : e[3] < 20 ? 1 : 2;
    gp[tier] += "M" + lon2px(s1[0]).toFixed(1) + " " + lat2px(s1[1]).toFixed(1) + "L" + lon2px(s2[0]).toFixed(1) + " " + lat2px(s2[1]).toFixed(1);
  }
  [0, 1, 2].forEach(function (t) { gZoom.appendChild(el("path", { d: gp[t], stroke: GREY[t].c, "stroke-width": GREY[t].w, "stroke-opacity": GREY[t].o, fill: "none", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke" })); });

  // --- Journeys: a single node travelling along each service's route. The route
  // itself isn't drawn (it lies on the grey network), so nothing overlaps. ---
  var jobjs = [];
  for (var ji = 0; ji < journeys.length; ji++) {
    var j = journeys[ji], pts = [];
    for (var pk = 0; pk < j.p.length; pk++) { var s = stations[j.p[pk]]; if (s) pts.push([lon2px(s[0]), lat2px(s[1])]); }
    if (pts.length < 2) continue;
    var cum = [0];
    for (var ck = 1; ck < pts.length; ck++) cum[ck] = cum[ck - 1] + Math.hypot(pts[ck][0] - pts[ck - 1][0], pts[ck][1] - pts[ck - 1][1]);
    var halo = el("circle", { fill: COL[j.b], "fill-opacity": "0", display: "none" });
    var core = el("circle", { fill: BRIGHT[j.b], "fill-opacity": "0", display: "none" });
    gZoom.appendChild(halo); gZoom.appendChild(core);
    var last = pts[pts.length - 1];
    jobjs.push({ j: j, pts: pts, cum: cum, total: cum[cum.length - 1], halo: halo, core: core, lx: last[0], ly: last[1], depMin: j.dep, durMin: j.dur });
  }

  // --- Pan / zoom (persisted across re-inits) ---
  var view = RP._view || { k: 1, tx: 0, ty: 0 };
  function clampPan() { view.tx = Math.max(B.w * (1 - view.k), Math.min(0, view.tx)); view.ty = Math.max(B.h * (1 - view.k), Math.min(0, view.ty)); }
  function applyT() { clampPan(); gZoom.setAttribute("transform", "translate(" + view.tx.toFixed(2) + "," + view.ty.toFixed(2) + ") scale(" + view.k.toFixed(4) + ")"); RP._view = view; }
  function toVB(cx, cy) { var pt = svg.createSVGPoint(); pt.x = cx; pt.y = cy; var m = svg.getScreenCTM(); if (!m) return [0, 0]; var p = pt.matrixTransform(m.inverse()); return [p.x, p.y]; }
  applyT();
  svg.addEventListener("wheel", function (ev) {
    ev.preventDefault();
    var v = toVB(ev.clientX, ev.clientY), lx = (v[0] - view.tx) / view.k, ly = (v[1] - view.ty) / view.k;
    view.k = Math.max(1, Math.min(9, view.k * (ev.deltaY < 0 ? 1.16 : 1 / 1.16)));
    view.tx = v[0] - view.k * lx; view.ty = v[1] - view.k * ly; applyT();
  }, { passive: false, signal: sig });
  var drag = null;
  svg.addEventListener("mousedown", function (ev) { drag = toVB(ev.clientX, ev.clientY); ev.preventDefault(); }, { signal: sig });
  window.addEventListener("mousemove", function (ev) { if (!drag) return; var v = toVB(ev.clientX, ev.clientY); view.tx += v[0] - drag[0]; view.ty += v[1] - drag[1]; drag = v; applyT(); }, { signal: sig });
  window.addEventListener("mouseup", function () { drag = null; }, { signal: sig });
  var fsBtn = document.getElementById("map-fs"), fsTarget = svg.parentNode;
  if (fsBtn && fsTarget) fsBtn.onclick = function () { if (document.fullscreenElement) { if (document.exitFullscreen) document.exitFullscreen(); } else if (fsTarget.requestFullscreen) fsTarget.requestFullscreen(); };

  // --- Banner cards (driven by the live journey animation) ---
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "on time"; }
  function jcard(j, settled) {
    var right = settled ? delayVal(j.dly) : "en route", sub = settled ? (j.b === 0 ? "on time" : j.b === 1 ? "slight" : "late") : "live";
    return '<div class="mini" style="--c:' + COL[j.b] + '"><span class="hc">' + esc(j.lbl) + '</span><span class="rt">' + esc(j.o) + ' → ' + esc(j.d) + '</span><span class="pa">' + right + '<small>' + sub + '</small></span></div>';
  }
  var trackBox = document.getElementById("rail-track"), settleBox = document.getElementById("rail-settled"), clockEl = document.getElementById("map-clock"), lastBanner = 0;
  var RT = 210000;     // ms to replay the full day
  var SETTLE_MIN = 9;  // replay-minutes a service lingers in "Just settled"
  function pad(n) { return (n < 10 ? "0" : "") + n; }
  function place(o, x, y, r, alpha) {
    o.core.setAttribute("cx", x.toFixed(1)); o.core.setAttribute("cy", y.toFixed(1)); o.core.setAttribute("r", r.toFixed(2)); o.core.setAttribute("fill-opacity", (0.98 * alpha).toFixed(2)); o.core.setAttribute("display", "");
    o.halo.setAttribute("cx", x.toFixed(1)); o.halo.setAttribute("cy", y.toFixed(1)); o.halo.setAttribute("r", (r * 2.3).toFixed(2)); o.halo.setAttribute("fill-opacity", (0.32 * alpha).toFixed(2)); o.halo.setAttribute("display", "");
  }

  function frame(now) {
    if (RP._t0 == null) RP._t0 = now;
    var clock = (((now - RP._t0) % RT) / RT) * 1440; // minutes since midnight (replay)
    var rb = 3.4 / view.k, track = [], settled = [];
    for (var i = 0; i < jobjs.length; i++) {
      var o = jobjs[i], end = o.depMin + o.durMin;
      if (clock >= o.depMin && clock < end) {
        // node position by arc length along the route (constant speed)
        var tp = (clock - o.depMin) / o.durMin, d = tp * o.total, si = 0;
        while (si < o.pts.length - 2 && o.cum[si + 1] < d) si++;
        var segLen = o.cum[si + 1] - o.cum[si], f = segLen > 0 ? (d - o.cum[si]) / segLen : 0;
        var a = o.pts[si], b = o.pts[si + 1];
        place(o, a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, rb, 1);
        track.push(o.j);
      } else if (clock >= end && clock < end + SETTLE_MIN) {
        var sf = (clock - end) / SETTLE_MIN;
        place(o, o.lx, o.ly, rb * (1 + 0.8 * (1 - sf)), 1 - sf); // small pop on arrival
        settled.push(o.j);
      } else {
        o.core.setAttribute("display", "none"); o.halo.setAttribute("display", "none");
      }
    }
    if (clockEl) clockEl.textContent = pad(Math.floor(clock / 60)) + ":" + pad(Math.floor(clock % 60));
    if (now - lastBanner > 250) {
      lastBanner = now;
      if (trackBox) trackBox.innerHTML = track.slice(0, 8).map(function (j) { return jcard(j, false); }).join("") || '<p class="empty">—</p>';
      if (settleBox) settleBox.innerHTML = settled.slice(0, 6).map(function (j) { return jcard(j, true); }).join("") || '<p class="empty">—</p>';
    }
    RP._raf = requestAnimationFrame(frame);
  }
  RP._raf = requestAnimationFrame(frame);
} };
