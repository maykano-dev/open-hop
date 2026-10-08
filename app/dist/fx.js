"use strict";
const T = window.__TAURI__;
const stage = document.getElementById("stage");
const label = document.getElementById("label");
let timer = null;
function play(p) {
  clearTimeout(timer);
  stage.className = "stage";
  void stage.offsetWidth; // restart the animations
  if (p.kind === "incoming") {
    label.textContent = p.from ? `From ${p.from}` : p.label;
    stage.classList.add("incoming");
    // If it never lands (very slow transfer), fade out eventually.
    timer = setTimeout(done, 20000);
  } else {
    label.textContent = p.label;
    stage.classList.add("landed");
    timer = setTimeout(done, 950);
  }
}
function done() {
  stage.className = "stage";
  label.textContent = "";
  if (T) T.core.invoke("fx_done");
}
if (T) {
  T.event.listen("fx", (e) => play(e.payload));
  T.core.invoke("snapshot").then((s) => window.OpenHopTheme.apply(s.config.ui_appearance, s.config.ui_accent)).catch(() => {});
} else {
  play({ kind: "incoming", label: "holiday.mp4", from: "macbook" });
  setTimeout(() => play({ kind: "landed", label: "holiday.mp4" }), 2200);
}
