// board.js — the shared live-board renderer.
//
// One renderer, two drivers: the /live page polls /ui/live/snapshot and feeds
// snapshots here; the standalone replay.html feeds recorded snapshots from a JSON
// file. Both call renderBoard(boardEl, snapshot) so live and replay look identical.
// Plain classic script (no ES module) so replay.html works over file:// too —
// `renderBoard` is a global, no build step.

const esc = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

function predChip(pred) {
  if (pred <= 0) return '<span class="pchip p-ontime">on time</span>';
  const cls = pred <= 5 ? "p-min" : "p-late";
  return `<span class="pchip ${cls}">+${pred}m</span>`;
}

function delayText(v, prefix) {
  return v <= 0 ? `${prefix} on time` : `${prefix} +${v}m`;
}

function accChip(delta) {
  const cls = delta <= 3 ? "acc-good" : delta <= 8 ? "acc-ok" : "acc-bad";
  return `<span class="acc ${cls}">Δ${delta}</span>`;
}

function trackCard(t) {
  return `<div class="tcard" data-rid="${esc(t.rid)}" data-op="${esc(t.operator)}" data-from="${esc(t.origin)}" style="--op:${esc(t.brand)}">
    <div class="tc-top"><span class="tc-hc">${esc(t.label)}</span><span class="tc-op">${esc(t.operator)}</span></div>
    <div class="tc-route"><code>${esc(t.origin)}</code><span class="ar">→</span><code>${esc(t.dest)}</code></div>
    <div class="tc-bot"><span class="tc-sched">dep ${esc(t.scheduled)}</span>${predChip(t.predicted)}</div>
  </div>`;
}

function settledCard(s) {
  return `<div class="tcard" data-rid="${esc(s.rid)}" data-op="${esc(s.operator)}" data-from="${esc(s.origin)}" style="--op:${esc(s.brand)}">
    <div class="tc-top"><span class="tc-hc">${esc(s.label)}</span><span class="tc-op">${esc(s.operator)}</span></div>
    <div class="tc-route"><code>${esc(s.origin)}</code><span class="ar">→</span><code>${esc(s.dest)}</code></div>
    <div class="tc-bot settled-row"><span class="pa pred">${delayText(s.predicted, "pred")}</span><span class="ar">→</span><span class="pa act">${delayText(s.actual, "actual")}</span>${accChip(s.delta)}</div>
  </div>`;
}

const emptyMsg = (m) => `<p class="panel-empty">${esc(m)}</p>`;

let activeFilter = { type: "all" };

function applyFilter(boardEl) {
  boardEl.querySelectorAll(".tcard").forEach((c) => {
    let show = true;
    if (activeFilter.type === "op") show = c.dataset.op === activeFilter.val;
    else if (activeFilter.type === "from") show = c.dataset.from === activeFilter.val;
    c.style.display = show ? "" : "none";
  });
}

// Render a snapshot into `boardEl`. Cards whose rid wasn't already on the board
// get `.fresh` so the CSS slides them in. Global (used by board polling + replay.js).
function renderBoard(boardEl, snap) {
  const existing = new Set([...boardEl.querySelectorAll("[data-rid]")].map((e) => e.dataset.rid));
  const tracking = (snap.tracking || []).map(trackCard).join("");
  const settled = (snap.settled || []).map(settledCard).join("");
  boardEl.innerHTML = `
    <div class="zone">
      <div class="zone-head"><h2>Tracking now</h2><span class="ct">${(snap.tracking || []).length} trains</span></div>
      <div class="tgrid">${tracking || emptyMsg("No trains tracking right now.")}</div>
    </div>
    <div class="zone">
      <div class="zone-head"><h2>Just settled</h2><span class="ct">predicted vs actual</span></div>
      <div class="tgrid">${settled || emptyMsg("Nothing settled yet.")}</div>
    </div>`;
  boardEl.querySelectorAll("[data-rid]").forEach((el) => {
    if (!existing.has(el.dataset.rid)) el.classList.add("fresh");
  });
  applyFilter(boardEl);
}

function chipEl(label, f) {
  const on = JSON.stringify(f) === JSON.stringify(activeFilter) ? " on" : "";
  return `<span class="chip${on}" data-f='${esc(JSON.stringify(f))}'>${esc(label)}</span>`;
}

function buildChips(boardEl, snap) {
  const filters = document.getElementById("filters");
  if (!filters) return;
  const all = [...(snap.tracking || []), ...(snap.settled || [])];
  const ops = [...new Set(all.map((x) => x.operator).filter((o) => o && o !== "—"))].slice(0, 5);
  const froms = [...new Set((snap.tracking || []).map((x) => x.origin).filter(Boolean))].slice(0, 4);
  let html = chipEl("All", { type: "all" });
  ops.forEach((o) => (html += chipEl(o, { type: "op", val: o })));
  froms.forEach((f) => (html += chipEl("from " + f, { type: "from", val: f })));
  filters.innerHTML = html;
  filters.querySelectorAll(".chip").forEach((ch) => {
    ch.onclick = () => {
      activeFilter = JSON.parse(ch.dataset.f);
      filters.querySelectorAll(".chip").forEach((x) => x.classList.remove("on"));
      ch.classList.add("on");
      applyFilter(boardEl);
    };
  });
}

// Live bootstrap — only runs on the /live page (which has the Record control).
// When board.js is imported by replay.html (no #record) this is a no-op.
function startLive() {
  const boardEl = document.getElementById("board");
  const recBtn = document.getElementById("record");
  const recLabel = document.getElementById("record-label");
  const dl = document.getElementById("download");
  const frames = [];
  let recording = false;
  let chipsDone = false;

  recBtn.onclick = () => {
    recording = !recording;
    recBtn.classList.toggle("on", recording);
    recLabel.textContent = recording ? `Recording… (${frames.length})` : "Record";
    if (!recording && frames.length) {
      const payload = { meta: { captured_at: Date.now(), app: "railpredict-live" }, frames };
      const blob = new Blob([JSON.stringify(payload)], { type: "application/json" });
      if (dl.href && dl.href.startsWith("blob:")) URL.revokeObjectURL(dl.href);
      dl.href = URL.createObjectURL(blob);
      dl.textContent = `Download (${frames.length})`;
      dl.classList.remove("hidden");
    }
  };

  async function tick() {
    try {
      const r = await fetch("/ui/live/snapshot", { cache: "no-store" });
      if (!r.ok) return;
      const snap = await r.json();
      renderBoard(boardEl, snap);
      const lc = document.getElementById("live-count");
      if (lc) lc.textContent = (snap.tracking || []).length;
      if (!chipsDone) {
        buildChips(boardEl, snap);
        chipsDone = true;
      }
      if (recording) {
        frames.push(snap);
        recLabel.textContent = `Recording… (${frames.length})`;
      }
    } catch (e) {
      // Transient fetch failure: keep the last board on screen rather than blanking it.
      console.debug("snapshot poll failed", e);
    }
  }

  tick();
  setInterval(tick, 3000);
}

if (document.getElementById("record")) {
  startLive();
}
