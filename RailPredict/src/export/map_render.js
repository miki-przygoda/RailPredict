// Exposed as RailPredictMap.init() so the standalone map page can call it on
// load and the OS shell can call it when the Map window opens.
//
// Basemap is a real OSM/CARTO snapshot (raster baked in) with known Web-Mercator
// bounds. On top: the rail network lit by average delay (green/amber/red), and
// real multi-stop journeys playing out — a train runs its full calling pattern
// (easing into each stop with a little bounce), its route lights up as it goes,
// and it appears in the "Tracking" banner while running, hops to "Just settled"
// on arrival, then clears. Scroll to zoom, drag to pan, ⛶ for fullscreen.
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
  var COL = ["#34d399", "#f2c14e", "#f04545"]; // 0 on-time, 1 slight, 2 late

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

  var img = el("image", { x: "0", y: "0", width: B.w, height: B.h, opacity: "0.72" });
  img.setAttributeNS(XLINK, "href", IMG);
  img.setAttribute("href", IMG);
  gZoom.appendChild(img);

  var N = Math.pow(2, B.z) * 256;
  function lon2px(lon) { return (lon + 180) / 360 * N - B.ox; }
  function lat2px(lat) { var r = lat * Math.PI / 180; return (1 - Math.asinh(Math.tan(r)) / Math.PI) / 2 * N - B.oy; }

  // --- Static lit network (avg delay per segment), glow then crisp ---
  var dp = ["", "", ""];
  for (var ei = 0; ei < edges.length; ei++) {
    var e = edges[ei], s1 = stations[e[0]], s2 = stations[e[1]];
    if (!s1 || !s2) continue;
    dp[e[2]] += "M" + lon2px(s1[0]).toFixed(1) + " " + lat2px(s1[1]).toFixed(1) +
                "L" + lon2px(s2[0]).toFixed(1) + " " + lat2px(s2[1]).toFixed(1);
  }
  [0, 1, 2].forEach(function (b) {
    gZoom.appendChild(el("path", { d: dp[b], stroke: COL[b], "stroke-width": "4.5", "stroke-opacity": "0.12", fill: "none", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke" }));
  });
  [0, 1, 2].forEach(function (b) {
    gZoom.appendChild(el("path", { d: dp[b], stroke: COL[b], "stroke-width": b === 2 ? "1.6" : "1.2", "stroke-opacity": b === 0 ? "0.5" : "0.72", fill: "none", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke" }));
  });

  // --- Multi-stop journeys: a lit route + a running train per journey ---
  var jobjs = [];
  for (var ji = 0; ji < journeys.length; ji++) {
    var j = journeys[ji];
    var pts = [];
    for (var pk = 0; pk < j.p.length; pk++) { var s = stations[j.p[pk]]; if (s) pts.push([lon2px(s[0]), lat2px(s[1])]); }
    if (pts.length < 2) continue;
    var dpath = "M" + pts.map(function (p) { return p[0].toFixed(1) + " " + p[1].toFixed(1); }).join("L");
    var route = el("path", { d: dpath, stroke: COL[j.b], "stroke-width": "2.2", "stroke-opacity": "0", fill: "none", "stroke-linecap": "round", "stroke-linejoin": "round", "vector-effect": "non-scaling-stroke" });
    var halo = el("circle", { fill: COL[j.b], "fill-opacity": "0.4", display: "none" });
    var core = el("circle", { fill: "#f6faff", "fill-opacity": "0.98", display: "none" });
    gZoom.appendChild(route); gZoom.appendChild(halo); gZoom.appendChild(core);
    var cum = [0];
    for (var ck = 1; ck < pts.length; ck++) cum[ck] = cum[ck - 1] + Math.hypot(pts[ck][0] - pts[ck - 1][0], pts[ck][1] - pts[ck - 1][1]);
    var total = cum[cum.length - 1];
    var travelDur = Math.max(11000, Math.min(32000, total * 28)); // constant pixel speed
    var settleDur = 3800, gapDur = 3000 + (ji % 6) * 850;
    var T = travelDur + settleDur + gapDur;
    jobjs.push({ j: j, pts: pts, cum: cum, total: total, route: route, halo: halo, core: core, travelDur: travelDur, settleDur: settleDur, T: T, offset: (ji * 3203) % T });
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
    var right = settled ? delayVal(j.dly) : "en route";
    var sub = settled ? (j.b === 0 ? "on time" : j.b === 1 ? "slight" : "late") : "live";
    return '<div class="mini" style="--c:' + COL[j.b] + '"><span class="hc">' + esc(j.lbl) + '</span><span class="rt">' + esc(j.o) + ' → ' + esc(j.d) + '</span><span class="pa">' + right + '<small>' + sub + '</small></span></div>';
  }
  var trackBox = document.getElementById("rail-track"), settleBox = document.getElementById("rail-settled"), lastBanner = 0;

  function place(o, x, y, r) {
    o.core.setAttribute("cx", x.toFixed(1)); o.core.setAttribute("cy", y.toFixed(1)); o.core.setAttribute("r", r.toFixed(2)); o.core.setAttribute("display", "");
    o.halo.setAttribute("cx", x.toFixed(1)); o.halo.setAttribute("cy", y.toFixed(1)); o.halo.setAttribute("r", (r * 2.2).toFixed(2)); o.halo.setAttribute("display", "");
  }

  function frame(now) {
    if (RP._t0 == null) RP._t0 = now;
    var elapsed = now - RP._t0, rb = 5 / view.k, track = [], settled = [];
    for (var i = 0; i < jobjs.length; i++) {
      var o = jobjs[i], local = (elapsed + o.offset) % o.T;
      if (local < o.travelDur) {
        // constant-speed position along the route (arc-length lookup)
        var tp = local / o.travelDur, d = tp * o.total, si = 0;
        while (si < o.pts.length - 2 && o.cum[si + 1] < d) si++;
        var segLen = o.cum[si + 1] - o.cum[si], f = segLen > 0 ? (d - o.cum[si]) / segLen : 0;
        var a = o.pts[si], b = o.pts[si + 1];
        var x = a[0] + (b[0] - a[0]) * f, y = a[1] + (b[1] - a[1]) * f;
        var bs = rb * (1 + 0.26 * Math.abs(Math.sin((now + o.offset) / 185))); // lively bounce
        o.route.setAttribute("stroke-opacity", (0.9 * Math.min(1, tp / 0.05)).toFixed(2));
        place(o, x, y, bs);
        track.push(o.j);
      } else if (local < o.travelDur + o.settleDur) {
        var sf = (local - o.travelDur) / o.settleDur, last = o.pts[o.pts.length - 1];
        o.route.setAttribute("stroke-opacity", (0.9 * (1 - sf)).toFixed(2));
        place(o, last[0], last[1], rb * (1 + 0.6 * (1 - sf)) * (1 - 0.3 * sf)); // pop on arrival, then settle
        settled.push(o.j);
      } else {
        o.route.setAttribute("stroke-opacity", "0"); o.core.setAttribute("display", "none"); o.halo.setAttribute("display", "none");
      }
    }
    if (now - lastBanner > 250) {
      lastBanner = now;
      if (trackBox) trackBox.innerHTML = track.slice(0, 7).map(function (j) { return jcard(j, false); }).join("") || '<p class="empty">—</p>';
      if (settleBox) settleBox.innerHTML = settled.slice(0, 5).map(function (j) { return jcard(j, true); }).join("") || '<p class="empty">—</p>';
    }
    RP._raf = requestAnimationFrame(frame);
  }
  RP._raf = requestAnimationFrame(frame);
} };
