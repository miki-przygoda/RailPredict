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
  // Live services (from /ui/map/snapshot) take over when present; otherwise fall
  // back to the baked yesterday-replay (used by the offline OS demo).
  var replayJourneys = window.MAP_JOURNEYS || [];   // [{p:[idx...],lbl,o,d,dly,b},...]
  // Only the live snapshot's services carry a `route`; the offline demo bakes a
  // routeless trains list, so require `route` to enter live mode (else replay).
  var liveTrains = (M && Array.isArray(M.trains) && M.trains.length && M.trains[0] && M.trains[0].route) ? M.trains : null;
  var journeys = liveTrains || replayJourneys;       // live: {route,dep,dur,dly,b,lbl,o,d}
  var liveMode = !!liveTrains;
  function fmtReplayDate(s) {
    var p = String(s).split("-");
    if (p.length !== 3) return "yesterday";
    var mo = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    return Number(p[2]) + " " + (mo[Number(p[1]) - 1] || "") + " " + p[0];
  }
  var clkLbl = document.querySelector(".map-clock-wrap .lbl");
  if (clkLbl) {
    var rDate = window.MAP_ABOUT && window.MAP_ABOUT.date;
    clkLbl.textContent = liveMode ? "Live · now" : ("Replaying " + (rDate ? fmtReplayDate(rDate) : "yesterday"));
  }
  var cntEl = document.getElementById("map-count");
  if (cntEl) {
    if (liveMode) {
      // Count services running *right now* (en route) — matches the dashboard's
      // "Live network" tally. Poised/upcoming nodes are shown but not counted.
      var nm = (function () { var d = new Date(); return d.getUTCHours() * 60 + d.getUTCMinutes() + d.getUTCSeconds() / 60; })();
      var running = 0;
      for (var ci = 0; ci < journeys.length; ci++) { var cj = journeys[ci]; if (nm >= cj.dep && nm < cj.dep + cj.dur) running++; }
      cntEl.textContent = running + " trains";
    } else cntEl.textContent = "";
  }
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
  // "meet" = contain: the whole of GB is visible at once (zoomed out to fit),
  // centred. Scroll to zoom in, then drag to pan around. (The map cell now
  // fills the full section — the div/`.cc-map` reset stops it collapsing — so
  // this fit view is large, just letterboxed left/right because GB is tall.)
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
  // Plain track network: every link the same flat 1px line, no weighting by
  // traffic or anything else. Built as a single path so overlapping segments
  // never compound into darker or thicker lines — just the track, with trains
  // running over it.
  var gp = "";
  for (var ei = 0; ei < edges.length; ei++) {
    var e = edges[ei], s1 = stations[e[0]], s2 = stations[e[1]];
    if (!s1 || !s2) continue;
    gp += "M" + lon2px(s1[0]).toFixed(1) + " " + lat2px(s1[1]).toFixed(1) + "L" + lon2px(s2[0]).toFixed(1) + " " + lat2px(s2[1]).toFixed(1);
  }
  gZoom.appendChild(el("path", { d: gp, stroke: "#7886a0", "stroke-width": "1", "stroke-opacity": "0.5", fill: "none", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke" }));

  // --- Click-to-inspect: a highlighted route for the selected train (drawn under
  // the moving nodes) + a right-rail detail panel that replaces the KPIs. The full
  // feature is gated on the panel existing (the OS demo); elsewhere it no-ops. ---
  var selPath = el("path", { d: "", fill: "none", stroke: "#fff", "stroke-width": "3.2", "stroke-linejoin": "round", "stroke-linecap": "round", "vector-effect": "non-scaling-stroke", "stroke-opacity": "0.95", display: "none" });
  gZoom.appendChild(selPath);
  var detailEl = document.getElementById("map-detail");
  var numbersEl = document.getElementById("map-numbers");
  var clickable = !!detailEl;
  var stCodes = window.MAP_STATION_CODES || [];
  var sel = null, selCalls = null, selProg = null, selStatus = null;

  // --- Journeys: a single node travelling along each service's route. The route
  // itself isn't drawn (it lies on the grey network), so nothing overlaps. ---
  var jobjs = [];
  for (var ji = 0; ji < journeys.length; ji++) {
    var j = journeys[ji], pts = [];
    if (j.route) {
      for (var pk = 0; pk < j.route.length; pk++) { var c = j.route[pk]; pts.push([lon2px(c[0]), lat2px(c[1])]); }
    } else {
      for (var pk = 0; pk < j.p.length; pk++) { var s = stations[j.p[pk]]; if (s) pts.push([lon2px(s[0]), lat2px(s[1])]); }
    }
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
  // Distinguish a click (select a train) from a drag (pan): a click is a
  // mouseup with negligible movement since mousedown.
  var drag = null, downXY = null, movedFar = false;
  svg.addEventListener("mousedown", function (ev) { drag = toVB(ev.clientX, ev.clientY); downXY = [ev.clientX, ev.clientY]; movedFar = false; ev.preventDefault(); }, { signal: sig });
  window.addEventListener("mousemove", function (ev) { if (!drag) return; if (downXY && Math.hypot(ev.clientX - downXY[0], ev.clientY - downXY[1]) > 4) movedFar = true; var v = toVB(ev.clientX, ev.clientY); view.tx += v[0] - drag[0]; view.ty += v[1] - drag[1]; drag = v; applyT(); }, { signal: sig });
  window.addEventListener("mouseup", function (ev) { if (drag && !movedFar && clickable) handleClick(ev.clientX, ev.clientY); drag = null; }, { signal: sig });
  var fsBtn = document.getElementById("map-fs"), fsTarget = svg.parentNode;
  if (fsBtn && fsTarget) fsBtn.onclick = function () { if (document.fullscreenElement) { if (document.exitFullscreen) document.exitFullscreen(); } else if (fsTarget.requestFullscreen) fsTarget.requestFullscreen(); };

  // --- Click-to-inspect helpers (all function declarations — hoisted) ---
  function hhmm(m) { m = ((m % 1440) + 1440) % 1440; return pad(Math.floor(m / 60)) + ":" + pad(Math.floor(m % 60)); }
  function delayTag(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "on time"; }
  function titleOp(s) {
    if (!s) return "";
    var t = String(s).toLowerCase().replace(/\b\w/g, function (c) { return c.toUpperCase(); });
    return t.replace(/\bGwr\b/g, "GWR").replace(/\bLner\b/g, "LNER").replace(/\bTfl\b/g, "TfL").replace(/\bScotrail\b/g, "ScotRail").replace(/\bC2c\b/g, "c2c");
  }
  function selKey(j) { return (j.lbl || "") + "|" + (j.o || "") + "|" + (j.d || "") + "|" + (j.dep || 0); }
  function handleClick(cx, cy) {
    var v = toVB(cx, cy), lx = (v[0] - view.tx) / view.k, ly = (v[1] - view.ty) / view.k;
    var best = null, bestD = 22 / view.k;
    for (var i = 0; i < jobjs.length; i++) {
      var o = jobjs[i];
      if (!o._active) continue;
      var d = Math.hypot(o._cx - lx, o._cy - ly);
      if (d < bestD) { bestD = d; best = o; }
    }
    if (best) selectJourney(best); else deselect();
  }
  function selectJourney(o) {
    sel = o;
    RP._selId = selKey(o.j);
    selPath.setAttribute("d", "M" + o.pts.map(function (p) { return p[0].toFixed(1) + " " + p[1].toFixed(1); }).join("L"));
    selPath.setAttribute("stroke", BRIGHT[o.j.b]);
    selPath.setAttribute("display", "");
    if (numbersEl) numbersEl.style.display = "none";
    if (detailEl) {
      detailEl.style.display = "";
      detailEl.innerHTML = detailHtml(o.j);
      var back = detailEl.querySelector(".md-back");
      if (back) back.addEventListener("click", deselect);
      selCalls = detailEl.querySelectorAll(".md-call");
      selProg = detailEl.querySelector(".md-prog-fill");
      selStatus = detailEl.querySelector(".md-status");
    }
  }
  function deselect() {
    sel = null; selCalls = selProg = selStatus = null; RP._selId = null;
    selPath.setAttribute("display", "none"); selPath.setAttribute("d", "");
    if (detailEl) detailEl.style.display = "none";
    if (numbersEl) numbersEl.style.display = "";
  }
  function detailHtml(j) {
    // Live services carry their calling-point codes inline; the baked OS journeys
    // carry station indices resolved against the codes table.
    var calls = j.calls || (j.p || []).map(function (idx) { return stCodes[idx] || "·"; });
    var lis = calls.map(function (c, i) { return '<li class="md-call" data-i="' + i + '">' + esc(c) + '</li>'; }).join("");
    var op = j.op ? (String(j.op).length > 4 ? titleOp(j.op) : String(j.op).toUpperCase()) : "";
    return (
      '<button class="md-back" type="button">← Back to network</button>' +
      '<div class="md-hc">' + esc(j.lbl || "") + ' · service</div>' +
      (op ? '<div class="md-op">' + esc(op) + '</div>' : '') +
      '<div class="md-route">' + esc(j.o) + '<span class="md-arr"> → </span>' + esc(j.d) + '</div>' +
      '<div class="md-meta"><span>dep ' + hhmm(j.dep) + '</span><span class="md-status"></span></div>' +
      '<div class="md-prog"><div class="md-prog-fill" style="background:' + COL[j.b] + '"></div></div>' +
      '<div class="md-calls-h">' + calls.length + ' calling points</div>' +
      '<ol class="md-calls">' + lis + '</ol>'
    );
  }
  function updateDetail(info) {
    if (selProg) selProg.style.width = Math.round(info.tp * 100) + "%";
    if (selStatus) {
      var tag = delayTag(sel.j.dly);
      selStatus.textContent = info.state === "track" ? ("running · " + tag) : info.state === "done" ? ("arrived · " + tag) : "departs soon";
    }
    // Highlight the current calling point by journey progress (robust whether the
    // route is index-based (baked) or a snapped polyline (live)).
    if (selCalls && selCalls.length) {
      var cur = info.state === "done" ? selCalls.length - 1 : Math.round(info.tp * (selCalls.length - 1));
      for (var k = 0; k < selCalls.length; k++) {
        selCalls[k].classList.toggle("passed", k < cur);
        selCalls[k].classList.toggle("now", k === cur && info.state !== "poised");
      }
    }
  }

  // Restore a prior selection across re-inits — the live /map page re-initialises
  // the renderer on every 20 s poll, which would otherwise wipe the open panel.
  if (clickable && RP._selId) {
    var restoreObj = null;
    for (var rj = 0; rj < jobjs.length; rj++) {
      if (selKey(jobjs[rj].j) === RP._selId) { restoreObj = jobjs[rj]; break; }
    }
    if (restoreObj) selectJourney(restoreObj); else deselect();
  }

  // --- Banner cards (driven by the live journey animation) ---
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function delayVal(v) { return v > 0 ? "+" + v + "m" : v < 0 ? v + "m" : "on time"; }
  function jcard(j, settled) {
    var right = settled ? delayVal(j.dly) : "en route", sub = settled ? (j.b === 0 ? "on time" : j.b === 1 ? "slight" : "late") : "live";
    return '<div class="mini" style="--c:' + COL[j.b] + '"><span class="hc">' + esc(j.lbl) + '</span><span class="rt">' + esc(j.o) + ' → ' + esc(j.d) + '</span><span class="pa">' + right + '<small>' + sub + '</small></span></div>';
  }
  // Render a rail list, capped, with a "+N more" line when there's overflow.
  function railHtml(arr, cap, settled, moreWord) {
    if (!arr.length) return '<p class="empty">—</p>';
    var h = arr.slice(0, cap).map(function (j) { return jcard(j, settled); }).join("");
    if (arr.length > cap) h += '<p style="font:600 9.5px ui-monospace,SFMono-Regular,Menlo,monospace;color:#94a3b8;padding:5px 2px 1px">+' + (arr.length - cap) + ' more ' + moreWord + '</p>';
    return h;
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
    // Shared replay epoch (also used by the Replay app) so every "Replaying
    // yesterday" surface sweeps the day in lockstep.
    if (window.__rpT0 == null) window.__rpT0 = now;
    // Live: real wall-clock minutes since UTC midnight (matches the server's
    // dep/dur). Replay: sweep the full day on a loop.
    var clock = liveMode
      ? (function () { var d = new Date(); return d.getUTCHours() * 60 + d.getUTCMinutes() + d.getUTCSeconds() / 60; })()
      : (((now - window.__rpT0) % RT) / RT) * 1440;
    var rb = 3.4 / view.k, LEAD_MIN = 20, track = [], settled = [];
    // If a train is selected but no longer animating, keep its panel on "arrived".
    var selInfo = sel ? { state: "done", tp: 1, si: sel.pts.length - 1 } : null;
    for (var i = 0; i < jobjs.length; i++) {
      var o = jobjs[i], end = o.depMin + o.durMin;
      if (liveMode && clock >= o.depMin - LEAD_MIN && clock < o.depMin) {
        // Poised at the origin in the ~20 min before departure: small + dim,
        // then it brightens and glides once it actually departs.
        place(o, o.pts[0][0], o.pts[0][1], rb * 0.8, 0.42);
        o._cx = o.pts[0][0]; o._cy = o.pts[0][1]; o._active = true;
        if (o === sel) selInfo = { state: "poised", tp: 0, si: 0 };
      } else if (clock >= o.depMin && clock < end) {
        // node position by arc length along the route (constant speed)
        var tp = (clock - o.depMin) / o.durMin, d = tp * o.total, si = 0;
        while (si < o.pts.length - 2 && o.cum[si + 1] < d) si++;
        var segLen = o.cum[si + 1] - o.cum[si], f = segLen > 0 ? (d - o.cum[si]) / segLen : 0;
        var a = o.pts[si], b = o.pts[si + 1];
        var nx = a[0] + (b[0] - a[0]) * f, ny = a[1] + (b[1] - a[1]) * f;
        place(o, nx, ny, rb, 1);
        o._cx = nx; o._cy = ny; o._active = true;
        track.push(o.j);
        if (o === sel) selInfo = { state: "track", tp: tp, si: si };
      } else if (clock >= end && clock < end + SETTLE_MIN) {
        var sf = (clock - end) / SETTLE_MIN;
        place(o, o.lx, o.ly, rb * (1 + 0.8 * (1 - sf)), 1 - sf); // small pop on arrival
        o._cx = o.lx; o._cy = o.ly; o._active = true;
        settled.push(o.j);
        if (o === sel) selInfo = { state: "done", tp: 1, si: o.pts.length - 1 };
      } else {
        o.core.setAttribute("display", "none"); o.halo.setAttribute("display", "none");
        o._active = false;
      }
    }
    if (clockEl) {
      if (liveMode) { var lt = new Date(); clockEl.textContent = pad(lt.getHours()) + ":" + pad(lt.getMinutes()); }
      else clockEl.textContent = pad(Math.floor(clock / 60)) + ":" + pad(Math.floor(clock % 60));
    }
    if (now - lastBanner > 250) {
      lastBanner = now;
      if (trackBox) trackBox.innerHTML = railHtml(track, 8, false, "running");
      if (settleBox) settleBox.innerHTML = railHtml(settled, 6, true, "settled");
      if (sel && detailEl && selInfo) updateDetail(selInfo);
    }
    RP._raf = requestAnimationFrame(frame);
  }
  RP._raf = requestAnimationFrame(frame);
} };
