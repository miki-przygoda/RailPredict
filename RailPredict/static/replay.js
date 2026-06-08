// replay.js — drives the shared board renderer (board.js) from a recorded capture
// JSON, on a clock. No server, no Darwin, no model. Classic script; uses the global
// renderBoard() defined by board.js.
(function () {
  let frames = [];
  let idx = 0;
  let timer = null;
  let speed = 1;

  const boardEl = document.getElementById("board");
  const fileInput = document.getElementById("file");
  const playBtn = document.getElementById("play");
  const speedSel = document.getElementById("speed");
  const status = document.getElementById("status");

  function stop() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
    playBtn.textContent = "Play";
  }

  function step() {
    if (idx >= frames.length) {
      stop();
      status.textContent = "End of replay";
      return;
    }
    renderBoard(boardEl, frames[idx]);
    status.textContent = `Frame ${idx + 1} / ${frames.length}`;
    idx += 1;
  }

  function play() {
    if (!frames.length) return;
    if (timer) {
      stop();
      return;
    }
    if (idx >= frames.length) idx = 0;
    playBtn.textContent = "Pause";
    timer = setInterval(step, 3000 / speed);
    step();
  }

  fileInput.addEventListener("change", (e) => {
    const f = e.target.files[0];
    if (!f) return;
    const reader = new FileReader();
    reader.onload = () => {
      try {
        const data = JSON.parse(reader.result);
        frames = Array.isArray(data) ? data : data.frames || [];
        idx = 0;
        stop();
        status.textContent = `${frames.length} frames loaded`;
        if (frames.length) renderBoard(boardEl, frames[0]);
      } catch (err) {
        status.textContent = "Invalid capture file";
        console.error("replay parse failed", err);
      }
    };
    reader.readAsText(f);
  });

  playBtn.addEventListener("click", play);
  speedSel.addEventListener("change", () => {
    speed = parseFloat(speedSel.value) || 1;
    if (timer) {
      stop();
      play();
    }
  });
})();
