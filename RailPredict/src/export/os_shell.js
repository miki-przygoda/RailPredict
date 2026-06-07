/**
 * os_shell.js — RailPredict OS window manager
 *
 * Vanilla JS, no external dependencies, no framework.
 * Manages windows, dock, desktop icons, and boots the map app.
 *
 * Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
 */
(function () {
  "use strict";

  /* ── App registry ─────────────────────────────────────────────── */
  var APPS = [
    { id: "map",         title: "Live Map",          icon: "🗺️",  kind: "map"  },
    { id: "operators",   title: "Operators",          icon: "🚆",  kind: "view" },
    { id: "predictions", title: "Predictions",        icon: "🎯",  kind: "view" },
    { id: "reliability", title: "Reliability",        icon: "📊",  kind: "view" },
    { id: "replay",      title: "Replay",             icon: "▶️",  kind: "view" },
    { id: "about",       title: "About RailPredict",  icon: "ⓘ",  kind: "view" }
  ];

  /* ── State ────────────────────────────────────────────────────── */
  var zTop = 100;             // z-index counter for focus management
  var windows = {};           // id → { el, bar, body, min, max, prevRect }

  /* ── Utility: find app definition ────────────────────────────── */
  function appDef(id) {
    for (var i = 0; i < APPS.length; i++) { if (APPS[i].id === id) return APPS[i]; }
    return null;
  }

  /* ── Focus a window (bring to front) ─────────────────────────── */
  function focusWin(id) {
    var w = windows[id];
    if (!w) return;
    w.el.style.zIndex = ++zTop;
  }

  /* ── Dock running-dot update ──────────────────────────────────── */
  function updateDockDot(id) {
    var dot = document.querySelector("#dock .dock-item[data-app='" + id + "']");
    if (!dot) return;
    var open = !!windows[id];
    dot.classList.toggle("running", open);
  }

  /* ── Default window geometry ──────────────────────────────────── */
  function defaultRect(id) {
    // Cascade offset so multiple windows don't perfectly stack
    var count = Object.keys(windows).length;
    var layer = document.getElementById("windows");
    var lw = layer ? layer.offsetWidth  : window.innerWidth;
    var lh = layer ? layer.offsetHeight : (window.innerHeight - 28 - 70);
    var w = Math.min(900, lw * 0.75);
    var h = Math.min(640, lh * 0.8);
    var x = 60 + count * 28;
    var y = 40 + count * 28;
    // Clamp so window doesn't open off-screen
    x = Math.min(x, lw - w - 20);
    y = Math.min(y, lh - h - 20);
    return { x: Math.max(x, 20), y: Math.max(y, 20), w: w, h: h };
  }

  /* ── Apply a rect object to a window element ──────────────────── */
  function applyRect(el, r) {
    el.style.left   = r.x + "px";
    el.style.top    = r.y + "px";
    el.style.width  = r.w + "px";
    el.style.height = r.h + "px";
  }

  /* ── Capture current rect from DOM ───────────────────────────── */
  function captureRect(el) {
    return {
      x: parseInt(el.style.left,  10) || 0,
      y: parseInt(el.style.top,   10) || 0,
      w: parseInt(el.style.width, 10) || 800,
      h: parseInt(el.style.height,10) || 600
    };
  }

  /* ── Maximise / restore ───────────────────────────────────────── */
  function toggleMax(id) {
    var w = windows[id];
    if (!w) return;
    if (w.isMax) {
      // Restore
      w.el.classList.remove("max");
      applyRect(w.el, w.prevRect);
      w.isMax = false;
    } else {
      // Save current rect then maximise
      w.prevRect = captureRect(w.el);
      w.el.classList.add("max");
      w.isMax = true;
    }
    focusWin(id);
  }

  /* ── Minimise ─────────────────────────────────────────────────── */
  function minimiseWin(id) {
    var w = windows[id];
    if (!w) return;
    w.el.classList.add("min");
    w.isMin = true;
    // Keep running dot — window still "open"
  }

  /* ── Restore minimised window ─────────────────────────────────── */
  function restoreWin(id) {
    var w = windows[id];
    if (!w) return;
    w.el.classList.remove("min");
    w.isMin = false;
    focusWin(id);
  }

  /* ── Close window ─────────────────────────────────────────────── */
  function closeWin(id) {
    var w = windows[id];
    if (!w) return;
    if (w.dragAbort) w.dragAbort.abort(); // remove this window's document drag listeners
    if (w.resizeAbort) w.resizeAbort.abort(); // ...and its resize listeners
    if (w.body && w.body._replayTimer) clearInterval(w.body._replayTimer); // stop replay cycling
    w.el.parentNode && w.el.parentNode.removeChild(w.el);
    delete windows[id];
    updateDockDot(id);
  }

  /* ── Drag logic ───────────────────────────────────────────────── */
  function makeDraggable(bar, el, getId) {
    var startX, startY, origX, origY, dragging = false;
    // One AbortController per window so the document-level listeners below are
    // removed when the window closes (no global-listener leak).
    var ac = new AbortController();
    var sig = ac.signal;

    bar.addEventListener("mousedown", function (e) {
      // Don't drag if clicking traffic-light buttons
      if (e.target.closest && e.target.closest(".traffic")) return;
      var id = getId();
      var w = windows[id];
      if (w && w.isMax) return; // Can't drag maximised window
      dragging = true;
      startX = e.clientX;
      startY = e.clientY;
      var r = captureRect(el);
      origX = r.x;
      origY = r.y;
      focusWin(id);
      e.preventDefault();
    }, { signal: sig });

    document.addEventListener("mousemove", function (e) {
      if (!dragging) return;
      var dx = e.clientX - startX;
      var dy = e.clientY - startY;
      var layer = document.getElementById("windows");
      var lw = layer ? layer.offsetWidth  : window.innerWidth;
      var lh = layer ? layer.offsetHeight : (window.innerHeight - 98);
      var newX = Math.max(-60, Math.min(origX + dx, lw - 60));
      var newY = Math.max(0,   Math.min(origY + dy, lh - 40));
      el.style.left = newX + "px";
      el.style.top  = newY + "px";
    }, { signal: sig });

    document.addEventListener("mouseup", function () {
      dragging = false;
    }, { signal: sig });

    return ac;
  }

  /* ── Resize logic (drag the bottom-right grip) ────────────────── */
  function makeResizable(handle, el, getId) {
    var startX, startY, origW, origH, resizing = false;
    var ac = new AbortController();
    var sig = ac.signal;

    handle.addEventListener("mousedown", function (e) {
      var w = windows[getId()];
      if (w && w.isMax) return; // can't resize a maximised window
      resizing = true;
      startX = e.clientX;
      startY = e.clientY;
      var r = captureRect(el);
      origW = r.w;
      origH = r.h;
      focusWin(getId());
      e.preventDefault();
      e.stopPropagation(); // don't also start a drag
    }, { signal: sig });

    document.addEventListener("mousemove", function (e) {
      if (!resizing) return;
      el.style.width  = Math.max(360, origW + (e.clientX - startX)) + "px";
      el.style.height = Math.max(240, origH + (e.clientY - startY)) + "px";
    }, { signal: sig });

    document.addEventListener("mouseup", function () {
      resizing = false;
    }, { signal: sig });

    return ac;
  }

  /* ── Create a window ──────────────────────────────────────────── */
  function createWindow(id, def) {
    var layer = document.getElementById("windows");
    if (!layer) return;

    // Build DOM
    var win = document.createElement("div");
    win.className = "win";
    win.setAttribute("data-win-id", id);

    // Title bar
    var bar = document.createElement("div");
    bar.className = "win-bar";

    // Traffic lights
    var traffic = document.createElement("div");
    traffic.className = "traffic";

    function makeBtn(cls, label, handler) {
      var btn = document.createElement("button");
      btn.className = cls;
      btn.setAttribute("aria-label", label);
      btn.setAttribute("title", label);
      var tt = document.createElement("span");
      tt.className = "tt";
      tt.setAttribute("aria-hidden", "true");
      tt.textContent = label === "Close" ? "✕" : label === "Minimise" ? "–" : "⤢";
      btn.appendChild(tt);
      btn.addEventListener("click", function (e) { e.stopPropagation(); handler(); });
      return btn;
    }

    traffic.appendChild(makeBtn("t-close", "Close",    function () { closeWin(id); }));
    traffic.appendChild(makeBtn("t-min",   "Minimise", function () { minimiseWin(id); }));
    traffic.appendChild(makeBtn("t-max",   "Maximise", function () { toggleMax(id); }));

    // Title
    var titleEl = document.createElement("div");
    titleEl.className = "win-title";
    titleEl.textContent = def.title;

    bar.appendChild(traffic);
    bar.appendChild(titleEl);

    // Body
    var body = document.createElement("div");
    body.className = "win-body";

    win.appendChild(bar);
    win.appendChild(body);

    // Focus on click anywhere in window
    win.addEventListener("mousedown", function () { focusWin(id); });

    // Resize grip (bottom-right corner)
    var grip = document.createElement("div");
    grip.className = "win-resize";
    grip.setAttribute("aria-label", "Resize");
    win.appendChild(grip);

    // Drag + resize — keep the AbortControllers so closeWin() can detach listeners
    var dragAbort = makeDraggable(bar, win, function () { return id; });
    var resizeAbort = makeResizable(grip, win, function () { return id; });

    // Double-click title bar to toggle max
    bar.addEventListener("dblclick", function (e) {
      if (e.target.closest && e.target.closest(".traffic")) return;
      toggleMax(id);
    });

    // Position
    var r = defaultRect(id);
    applyRect(win, r);
    win.style.zIndex = ++zTop;

    layer.appendChild(win);

    windows[id] = {
      el: win, bar: bar, body: body,
      isMin: false, isMax: false, prevRect: r,
      dragAbort: dragAbort, resizeAbort: resizeAbort
    };

    updateDockDot(id);
    return windows[id];
  }

  /* ── Populate map window body ─────────────────────────────────── */
  function populateMap(body) {
    var tpl = document.getElementById("app-map");
    if (!tpl) {
      body.innerHTML = '<div style="padding:24px;color:#64748b;font-family:system-ui,sans-serif">Map template not found.</div>';
      return false;
    }
    // Clone template content into body
    var content = document.importNode(tpl.content, true);
    body.appendChild(content);
    return true;
  }

  /* ── Open or focus an app ─────────────────────────────────────── */
  function openApp(id) {
    var def = appDef(id);
    if (!def) return;

    // If already open: focus (and un-minimise)
    if (windows[id]) {
      restoreWin(id);
      return;
    }

    var w = createWindow(id, def);
    if (!w) return;

    if (def.kind === "map") {
      // Map window: maximise immediately
      w.prevRect = captureRect(w.el);
      w.el.classList.add("max");
      w.isMax = true;

      var populated = populateMap(w.body);

      if (populated && window.RailPredictMap && window.RailPredictMap.init) {
        // Wait for layout before calling init (SVG parent needs a real size)
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            // Guard: ensure map window is still open
            if (!windows[id]) return;
            try {
              window.RailPredictMap.init();
            } catch (e) {
              // Fail gracefully — map may have no data yet
              if (typeof console !== "undefined" && console.warn) {
                console.warn("RailPredictMap.init() failed:", e);
              }
            }
          });
        });
      }
    } else if (def.kind === "view") {
      // Windowed view: clone template and call OsApps initialiser
      var tplId = "app-" + id;
      var tpl = document.getElementById(tplId);
      var initialised = false;

      if (tpl && window.OsApps && typeof window.OsApps[id] === "function") {
        var content = document.importNode(tpl.content, true);
        w.body.appendChild(content);
        // Wait for layout before calling the initialiser (some views measure DOM)
        requestAnimationFrame(function () {
          requestAnimationFrame(function () {
            if (!windows[id]) return; // window may have been closed already
            try {
              window.OsApps[id](w.body);
            } catch (e) {
              if (typeof console !== "undefined" && console.warn) {
                console.warn("OsApps." + id + "() failed:", e);
              }
            }
          });
        });
        initialised = true;
      }

      if (!initialised) {
        // Fallback: coming soon placeholder (template or OsApps entry missing)
        w.body.innerHTML =
          '<div class="soon-body">' +
            '<div class="soon-icon">' + def.icon + '</div>' +
            '<div class="soon-title">' + def.title + '</div>' +
            '<div class="soon-sub">Coming soon — part of the RailPredict suite</div>' +
            '<div class="soon-tag">Stage 4</div>' +
          '</div>';
      }
    } else {
      // Explicit "soon" fallback (kind === "soon")
      w.body.innerHTML =
        '<div class="soon-body">' +
          '<div class="soon-icon">' + def.icon + '</div>' +
          '<div class="soon-title">' + def.title + '</div>' +
          '<div class="soon-sub">Coming soon — part of the RailPredict suite</div>' +
          '<div class="soon-tag">Stage 4</div>' +
        '</div>';
    }
  }

  /* ── Build desktop icons ──────────────────────────────────────── */
  function buildDesktopIcons() {
    var container = document.getElementById("desktop-icons");
    if (!container) return;
    APPS.forEach(function (app) {
      var icon = document.createElement("div");
      icon.className = "desk-icon";
      icon.setAttribute("data-app", app.id);
      icon.innerHTML =
        '<span class="icon-glyph">' + app.icon + '</span>' +
        '<span class="icon-label">' + app.title + '</span>';
      icon.addEventListener("click",    function () { openApp(app.id); });
      icon.addEventListener("dblclick", function () { openApp(app.id); });
      container.appendChild(icon);
    });
  }

  /* ── Build dock ───────────────────────────────────────────────── */
  function buildDock() {
    var dock = document.getElementById("dock");
    if (!dock) return;
    APPS.forEach(function (app, i) {
      if (i === 1) {
        // Divider before secondary apps
        var div = document.createElement("div");
        div.className = "dock-divider";
        dock.appendChild(div);
      }
      var item = document.createElement("div");
      item.className = "dock-item";
      item.setAttribute("data-app", app.id);
      item.innerHTML =
        '<span class="d-glyph">' + app.icon + '</span>' +
        '<div class="d-dot"></div>';
      item.setAttribute("title", app.title);
      item.addEventListener("click", function () {
        if (windows[app.id] && windows[app.id].isMin) {
          restoreWin(app.id);
        } else {
          openApp(app.id);
        }
      });
      dock.appendChild(item);
    });
  }

  /* ── Clock ────────────────────────────────────────────────────── */
  function startClock() {
    var el = document.getElementById("os-clock");
    if (!el) return;
    function tick() {
      var now = new Date();
      var h = String(now.getHours()).padStart(2, "0");
      var m = String(now.getMinutes()).padStart(2, "0");
      var s = String(now.getSeconds()).padStart(2, "0");
      var day = ["Sun","Mon","Tue","Wed","Thu","Fri","Sat"][now.getDay()];
      el.textContent = day + " " + h + ":" + m + ":" + s;
    }
    tick();
    setInterval(tick, 1000);
  }

  /* ── Boot ─────────────────────────────────────────────────────── */
  function boot() {
    buildDesktopIcons();
    buildDock();
    startClock();
    openApp("map");
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }

}());
