"use strict";
// A live window from another computer. It should feel like the window
// itself: same size, sharp, its own title bar, and your clicks and typing go
// to it. Only the parts of the picture that change are sent and redrawn.
// Drag it by its title bar to move it; carry it off the edge of the screen
// to send it on to another computer, or back home.

const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : null;
window.OPENHOP_DEBUG = false;
const q = new URLSearchParams(location.search);
const stream = q.get("s");
const origin = q.get("o") || "the other computer";
const canvas = document.getElementById("view");
const ctx = canvas.getContext("2d", { alpha: false });
const pill = document.getElementById("pill");
let fw = 0, fh = 0;          // picture size (the other computer's pixels)
let bar = 0;                 // title bar height, in picture pixels
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
    const w = u32(dv, o), h = u32(dv, o + 4), b = u32(dv, o + 8); o += 12;
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
    bar = b;
    let drawn = 0;
    patches.forEach((p, k) => {
      if (bitmaps[k]) { ctx.drawImage(bitmaps[k], p.x, p.y); bitmaps[k].close(); drawn++; }
    });
    if (window.OPENHOP_DEBUG) invoke("viewer_log", { msg: `update ${seq} ${w}x${h}: ${drawn}/${patches.length} drawn, canvas ${canvas.width}x${canvas.height}, view ${innerWidth}x${innerHeight}` }).catch(() => {});
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
// Maximized on this screen (the window's own maximize button or a double
// click on its title bar): the view fills the screen and the real window
// takes this size, like an app opened here.
let maxed = false;
// Right after maximizing or restoring, pictures at the old size are still on
// their way: don't let them size the view.
let settleUntil = 0;
if (window.__TAURI__) window.__TAURI__.event.listen("maximized", (e) => {
  maxed = !!e.payload;
  settleUntil = Date.now() + 1500;
  // Tell the real window this view's new size.
  setTimeout(() => { asked = null; fitIfNeeded(true); }, 350);
});

function resizeTo(w, h) {
  fw = w; fh = h;
  canvas.width = w; canvas.height = h;
  // Ours: the size we asked for, or the same shape smaller (the real window
  // can't be bigger than its own screen; the view then shows it larger).
  const ours = asked && Math.abs(asked.w / asked.h - w / h) < 0.03 && w <= asked.w + 3;
  if (!ours && !maxed && Date.now() > settleUntil && shown && invoke) {
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
        // Focused before the first picture came: say so now (typing goes
        // straight to the window from then on).
        if (document.hasFocus()) focusNow();
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
function fitIfNeeded(force) {
  if (!fw) return;
  const w = Math.round(window.innerWidth), h = Math.round(window.innerHeight);
  if (force || Math.abs(w - fw) > 3 || Math.abs(h - fh) > 3) {
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

// ---- moving and resizing the view itself

const win = window.__TAURI__ && window.__TAURI__.window ? window.__TAURI__.window.getCurrentWindow() : null;
const EDGE = 5; // px around the view that resize it
// Pressed on the title bar: a click goes to the window (its buttons work),
// a drag moves the view.
let barPress = null;
let moving = false;

function edgeAt(e) {
  const w = window.innerWidth, h = window.innerHeight;
  const l = e.clientX < EDGE, r = e.clientX >= w - EDGE, t = e.clientY < EDGE, b = e.clientY >= h - EDGE;
  return (t ? "North" : b ? "South" : "") + (l ? "West" : r ? "East" : "") || null;
}
const CURSORS = { North: "ns-resize", South: "ns-resize", East: "ew-resize", West: "ew-resize", NorthWest: "nwse-resize", SouthEast: "nwse-resize", NorthEast: "nesw-resize", SouthWest: "nesw-resize" };

function inBar(p) {
  // Unknown title bar (0): the top 32 pixels still move the view.
  const h = bar > 0 ? bar : Math.round(32 * fh / Math.max(1, box().h));
  return p.inside && p.y < h;
}

function stopMoving() {
  if (moving) {
    moving = false;
    if (invoke) invoke("viewer_drag", { on: false }).catch(() => {});
  }
}

// ---- input

let pendingMove = null;
canvas.addEventListener("mousemove", (e) => {
  if (e.buttons === 0) stopMoving();
  const edge = edgeAt(e);
  canvas.style.cursor = edge ? CURSORS[edge] : "default";
  const p = toFrame(e);
  if (barPress) {
    if (!moving && (e.buttons & 1) && Math.hypot(e.clientX - barPress.cx, e.clientY - barPress.cy) > 4) {
      moving = true;
      barPress = null;
      if (invoke) invoke("viewer_drag", { on: true }).catch(() => {});
    }
    return;
  }
  if (!p.inside) return;
  if (!pendingMove) requestAnimationFrame(() => { send({ Move: pendingMove }); pendingMove = null; });
  pendingMove = { x: p.x, y: p.y };
});
canvas.addEventListener("mousedown", (e) => {
  stopMoving();
  if (!focusSent) focusNow();
  const p = toFrame(e);
  canvas.focus();
  e.preventDefault();
  const edge = edgeAt(e);
  if (edge && e.button === 0 && win) {
    win.startResizeDragging(edge).catch(() => {});
    return;
  }
  if (!p.inside) return;
  if (e.button === 0 && inBar(p)) {
    barPress = { cx: e.clientX, cy: e.clientY, x: p.x, y: p.y };
    return;
  }
  send({ Button: { button: BUTTONS[e.button] || "Left", down: true, x: p.x, y: p.y } });
});
window.addEventListener("mouseup", (e) => {
  stopMoving();
  if (barPress) {
    // A click on the title bar (or its buttons): pass it on.
    const { x, y } = barPress;
    barPress = null;
    send({ Button: { button: "Left", down: true, x, y } });
    send({ Button: { button: "Left", down: false, x, y } });
    return;
  }
  const p = toFrame(e);
  send({ Button: { button: BUTTONS[e.button] || "Left", down: false, x: p.x, y: p.y } });
});
canvas.addEventListener("dblclick", (e) => e.preventDefault());
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
// Scrolling: a touchpad sends dozens of tiny steps a second; add them up
// and send once per screen refresh.
let wheel = null;
canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  const p = toFrame(e);
  const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 800 : 1;
  if (!wheel) {
    wheel = { dx: 0, dy: 0, x: p.x, y: p.y };
    requestAnimationFrame(() => {
      const w = wheel;
      wheel = null;
      const dx = Math.round(w.dx), dy = Math.round(w.dy);
      if (dx || dy) send({ Wheel: { dx, dy, x: w.x, y: w.y } });
    });
  }
  wheel.dy += -e.deltaY * unit * 1.2;
  wheel.dx += e.deltaX * unit * 1.2;
  wheel.x = p.x; wheel.y = p.y;
}, { passive: false });

const held = new Set();
window.addEventListener("keydown", (e) => {
  if (!focusSent) focusNow();
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
// Typing goes to the real window while this view has the focus.
let focusSent = false;
function focusNow() {
  if (!fw) return;
  focusSent = true;
  send("Focus");
}
window.addEventListener("focus", () => { stopMoving(); focusNow(); });
window.addEventListener("blur", () => {
  focusSent = false;
  send("Blur");
  // Don't leave keys stuck down on the other computer.
  for (const key of held) send({ Key: { key, down: false } });
  held.clear();
});

if (invoke) {
  invoke("snapshot").then((s) => window.OpenHopTheme.apply(null, s.config.ui_accent, s.system_dark)).catch(() => {});
}
