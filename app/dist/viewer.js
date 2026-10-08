"use strict";
// A live window from another computer: shows its pictures and sends your
// clicks, scrolling and typing back to it.

const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : null;
const q = new URLSearchParams(location.search);
const stream = q.get("s");
const origin = q.get("o") || "the other computer";
const canvas = document.getElementById("view");
const ctx = canvas.getContext("2d");
const pill = document.getElementById("pill");
let frame = null;           // ImageBitmap
let fw = 0, fh = 0;         // picture size (the other computer's pixels)
let box = { x: 0, y: 0, w: 1, h: 1 };
let closed = false;
let arrived = false;

function status(text, sub = "", loading = false) {
  document.getElementById("pillText").textContent = text;
  document.getElementById("pillSub").textContent = sub;
  pill.classList.toggle("loading", loading);
  pill.classList.toggle("hidden", !text);
}
status("Opening the window…", `from ${origin}`, true);

function draw() {
  const dpr = window.devicePixelRatio || 1;
  const cw = Math.round(canvas.clientWidth * dpr), ch = Math.round(canvas.clientHeight * dpr);
  if (canvas.width !== cw || canvas.height !== ch) { canvas.width = cw; canvas.height = ch; }
  ctx.fillStyle = getComputedStyle(document.body).backgroundColor;
  ctx.fillRect(0, 0, cw, ch);
  if (!frame) return;
  const k = Math.min(cw / fw, ch / fh);
  const w = fw * k, h = fh * k;
  box = { x: (cw - w) / 2 / dpr, y: (ch - h) / 2 / dpr, w: w / dpr, h: h / dpr };
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(frame, (cw - w) / 2, (ch - h) / 2, w, h);
}
window.addEventListener("resize", draw);

async function loop() {
  let after = "0";
  while (!closed && invoke) {
    let buf;
    try {
      buf = await invoke("win_frame", { stream, after });
    } catch (e) {
      status("OpenHop stopped", String(e));
      await new Promise((r) => setTimeout(r, 1000));
      continue;
    }
    if (!(buf instanceof ArrayBuffer)) buf = ArrayBuffer.isView(buf) ? buf.buffer : new Uint8Array(buf).buffer;
    const dv = new DataView(buf);
    const kind = dv.getUint8(0);
    const seq = dv.getBigUint64(1, true);
    const w = dv.getUint32(9, true), h = dv.getUint32(13, true);
    const tl = dv.getUint16(17, true);
    const text = new TextDecoder().decode(new Uint8Array(buf, 19, tl));
    if (kind === 0) {
      const jpeg = new Blob([new Uint8Array(buf, 19 + tl)], { type: "image/jpeg" });
      try {
        const bmp = await createImageBitmap(jpeg);
        if (frame) frame.close();
        frame = bmp; fw = w; fh = h;
        after = seq.toString();
        if (text) document.title = `${text} — on ${origin}`;
        draw();
        status("");
        if (!arrived) { arrived = true; canvas.classList.add("arrive"); canvas.focus(); }
      } catch (e) { status("Couldn't show the picture", String(e)); }
    } else if (kind === 1) {
      status(text || "Paused", "Plain mouse and keyboard sharing keeps working.");
      await new Promise((r) => setTimeout(r, 400));
    } else if (kind === 2) {
      closed = true;
      status("This window was closed", `on ${origin}`);
    }
  }
}
loop().catch((e) => status("Something went wrong", String(e)));

// ------------------------------------------------------------------ input

function toFrame(e) {
  const x = Math.round((e.clientX - box.x) * fw / box.w);
  const y = Math.round((e.clientY - box.y) * fh / box.h);
  return { x: Math.max(0, Math.min(fw - 1, x)), y: Math.max(0, Math.min(fh - 1, y)), inside: x >= 0 && y >= 0 && x < fw && y < fh };
}
function send(ev) {
  if (invoke && !closed && fw) invoke("win_input", { stream, ev }).catch(() => {});
}
const BUTTONS = ["Left", "Middle", "Right", "Back", "Forward"];

let pendingMove = null;
canvas.addEventListener("mousemove", (e) => {
  const p = toFrame(e);
  if (!p.inside) return;
  if (!pendingMove) requestAnimationFrame(() => { send({ Move: pendingMove }); pendingMove = null; });
  pendingMove = { x: p.x, y: p.y };
});
canvas.addEventListener("mousedown", (e) => {
  const p = toFrame(e);
  if (!p.inside) return;
  canvas.focus();
  e.preventDefault();
  send({ Button: { button: BUTTONS[e.button] || "Left", down: true, x: p.x, y: p.y } });
});
window.addEventListener("mouseup", (e) => {
  const p = toFrame(e);
  send({ Button: { button: BUTTONS[e.button] || "Left", down: false, x: p.x, y: p.y } });
});
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  const p = toFrame(e);
  const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 800 : 1;
  const dy = -Math.round(e.deltaY * unit * 1.2), dx = Math.round(e.deltaX * unit * 1.2);
  if (dx || dy) send({ Wheel: { dx, dy, x: p.x, y: p.y } });
}, { passive: false });

const held = new Set();
window.addEventListener("keydown", (e) => {
  const key = window.HID[e.code];
  if (key == null) return;
  e.preventDefault();
  // The other computer repeats held keys by itself.
  if (e.repeat) return;
  held.add(key);
  send({ Key: { key, down: true } });
});
window.addEventListener("keyup", (e) => {
  const key = window.HID[e.code];
  if (key == null) return;
  e.preventDefault();
  held.delete(key);
  send({ Key: { key, down: false } });
});
window.addEventListener("focus", () => send("Focus"));
window.addEventListener("blur", () => {
  send("Blur");
  // Don't leave keys stuck down on the other computer.
  for (const key of held) send({ Key: { key, down: false } });
  held.clear();
});

if (invoke) {
  invoke("snapshot").then((s) => window.OpenHopTheme.apply(s.config.ui_appearance, s.config.ui_accent)).catch(() => {});
}
