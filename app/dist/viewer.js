"use strict";
// A live window from another computer. It should feel like the window
// itself: same size, sharp, and your clicks and typing go to it. Only the
// parts of the picture that change are sent and redrawn.

const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : null;
const q = new URLSearchParams(location.search);
const stream = q.get("s");
const origin = q.get("o") || "the other computer";
const canvas = document.getElementById("view");
const ctx = canvas.getContext("2d", { alpha: false });
const pill = document.getElementById("pill");
let fw = 0, fh = 0;          // picture size (the other computer's pixels)
let closed = false;
let shown = false;
let title = "";
// A size we asked the real window to take (so its new pictures don't make
// us resize back).
let asked = null;

function status(text, sub = "", loading = false) {
  document.getElementById("pillText").textContent = text;
  document.getElementById("pillSub").textContent = sub;
  pill.classList.toggle("loading", loading);
  pill.classList.toggle("hidden", !text);
}
status("Opening the window…", `from ${origin}`, true);

// Where the picture is drawn inside the view (CSS pixels).
function box() {
  const r = canvas.getBoundingClientRect();
  if (!fw) return { x: 0, y: 0, w: r.width, h: r.height };
  const k = Math.min(r.width / fw, r.height / fh);
  const w = fw * k, h = fh * k;
  return { x: r.left + (r.width - w) / 2, y: r.top + (r.height - h) / 2, w, h };
}

function u32(dv, o) { return dv.getUint32(o, true); }

async function apply(buf) {
  const dv = new DataView(buf);
  let o = 1;
  const count = dv.getUint16(o, true); o += 2;
  let last = null;
  for (let i = 0; i < count; i++) {
    const seq = dv.getBigUint64(o, true); o += 8;
    const w = u32(dv, o), h = u32(dv, o + 4); o += 8;
    const tl = dv.getUint16(o, true); o += 2;
    const t = new TextDecoder().decode(new Uint8Array(buf, o, tl)); o += tl;
    const n = dv.getUint16(o, true); o += 2;
    const patches = [];
    for (let k = 0; k < n; k++) {
      const x = u32(dv, o), y = u32(dv, o + 4), pw = u32(dv, o + 8), ph = u32(dv, o + 12), len = u32(dv, o + 16);
      o += 20;
      patches.push({ x, y, pw, ph, blob: new Blob([new Uint8Array(buf, o, len)], { type: "image/jpeg" }) });
      o += len;
    }
    // Decode all patches first, then draw together (no half-drawn frames).
    const bitmaps = await Promise.all(patches.map((p) => createImageBitmap(p.blob).catch(() => null)));
    if (w !== fw || h !== fh) resizeTo(w, h);
    patches.forEach((p, k) => {
      if (bitmaps[k]) { ctx.drawImage(bitmaps[k], p.x, p.y); bitmaps[k].close(); }
    });
    if (t && t !== title) {
      title = t;
      document.title = t;
      invoke("viewer_title", { title: t }).catch(() => {});
    }
    last = seq;
  }
  return last;
}

// The picture changed size (the real window was resized over there).
function resizeTo(w, h) {
  fw = w; fh = h;
  canvas.width = w; canvas.height = h;
  const ours = asked && Math.abs(asked.w - w) <= 3 && Math.abs(asked.h - h) <= 3;
  if (!ours && shown && invoke) {
    // Follow it, so the view stays the same size as the window.
    invoke("viewer_fit", { w, h }).catch(() => {});
  }
  asked = null;
}

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
    const kind = new DataView(buf).getUint8(0);
    if (kind === 0) {
      const last = await apply(buf);
      if (last != null) after = last.toString();
      if (!shown) {
        shown = true;
        status("");
        canvas.focus();
        fitIfNeeded();
      } else {
        status("");
      }
    } else if (kind === 1) {
      const dv = new DataView(buf);
      const tl = dv.getUint16(1, true);
      status(new TextDecoder().decode(new Uint8Array(buf, 3, tl)) || "Paused", "Plain mouse and keyboard sharing keeps working.");
      await new Promise((r) => setTimeout(r, 400));
    } else if (kind === 2) {
      closed = true;
      status("This window was closed", `on ${origin}`);
    }
  }
}
loop().catch((e) => status("Something went wrong", String(e)));

// ------------------------------------------------------------------ size

// When this view is resized, resize the real window to match, so the
// picture stays sharp (1:1) instead of being stretched.
let resizeTimer = null;
function fitIfNeeded() {
  if (!fw) return;
  const w = Math.round(window.innerWidth), h = Math.round(window.innerHeight);
  if (Math.abs(w - fw) > 3 || Math.abs(h - fh) > 3) {
    asked = { w, h };
    send({ Resize: { w, h } });
  }
}
window.addEventListener("resize", () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(fitIfNeeded, 250);
});

// ------------------------------------------------------------------ input

function toFrame(e) {
  const b = box();
  const x = Math.round((e.clientX - b.x) * fw / b.w);
  const y = Math.round((e.clientY - b.y) * fh / b.h);
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
