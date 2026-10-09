"use strict";
// The island: a slim black pill at the top of the screen. Hover it (or
// press Ctrl+Alt+Space) and it springs open into the Control Center; short
// live activities (a laptop running low, files arriving) pop out of it.

const T = window.__TAURI__;
const invoke = T ? T.core.invoke : demo();
const $ = (id) => document.getElementById(id);
const island = $("island");

const ICON = {
  moon: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M20.2 14.6A8.6 8.6 0 0 1 9.4 3.8a8.6 8.6 0 1 0 10.8 10.8z"/></svg>',
  find: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="7"/><circle cx="12" cy="12" r="2.2" fill="currentColor" stroke="none"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3"/></svg>',
  lock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><rect x="5" y="10.5" width="14" height="10" rx="2.6" fill="currentColor" stroke="none"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/></svg>',
  power: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 3v8"/><path d="M6.6 6.6a7.6 7.6 0 1 0 10.8 0"/></svg>',
  gear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>',
  laptop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="5" width="16" height="11" rx="1.6"/><path d="M2 19h20"/></svg>',
  desktop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="1.6"/><path d="M9 20h6M12 16v4"/></svg>',
  battery: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="2.5" y="7" width="17" height="10" rx="2.5"/><rect x="4.6" y="9.1" width="4" height="5.8" rx="1" fill="currentColor" stroke="none"/><path d="M21.5 10.5v3" stroke-linecap="round"/></svg>',
  bolt: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M13.5 2 4 13.5h6.5L9.5 22 20 9.8h-6.6z"/></svg>',
  files: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M7 3h7l5 5v13H7z"/><path d="M14 3v5h5"/></svg>',
  pin: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s7-6.3 7-12a7 7 0 0 0-14 0c0 5.7 7 12 7 12z"/><circle cx="12" cy="10" r="2.5"/></svg>',
  search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m20 20-4.6-4.6"/></svg>',
  text: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M5 6h14M5 11h14M5 16h9"/></svg>',
  image: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="9" cy="10" r="2"/><path d="m4 18 5-5 4 4 3-3 5 5"/></svg>',
  pinned: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M15.5 3.5 20.5 8.5l-2.3.8-3.6 3.6.4 4.3-1.7 1.7-3.6-3.6L5 20l-1-1 4.7-4.7-3.6-3.6 1.7-1.7 4.3.4 3.6-3.6z"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M10 7V4.5h4V7M6.5 7l1 13h9l1-13"/></svg>',
  x: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3.4" stroke-linecap="round"><path d="m6 6 12 12M18 6 6 18"/></svg>',
  tray: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 13.5 5.6 5.8A2 2 0 0 1 7.5 4.5h9a2 2 0 0 1 1.9 1.3L21 13.5V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M3 13.5h5l1.5 2.5h5l1.5-2.5h5"/></svg>',
  all: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="7" width="12" height="9" rx="1.4"/><rect x="10" y="3" width="12" height="9" rx="1.4" fill="#000"/><path d="M6 20h4M8 16v4"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>',
  note: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M19 3.2v11.6a3.2 3.2 0 1 1-2-3V7.3l-8 1.8v7.7a3.2 3.2 0 1 1-2-3V5.4z"/></svg>',
  play: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.6v14.8a1 1 0 0 0 1.5.86l12.3-7.4a1 1 0 0 0 0-1.72L8.5 3.74A1 1 0 0 0 7 4.6z"/></svg>',
  pause: '<svg viewBox="0 0 24 24" fill="currentColor"><rect x="5.5" y="4" width="4.6" height="16" rx="1.4"/><rect x="13.9" y="4" width="4.6" height="16" rx="1.4"/></svg>',
  next: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M2.5 6.2v11.6a.8.8 0 0 0 1.25.66L12 12.66v5.14a.8.8 0 0 0 1.25.66l8.6-5.8a.8.8 0 0 0 0-1.32l-8.6-5.8A.8.8 0 0 0 12 6.2v5.14L3.75 5.54a.8.8 0 0 0-1.25.66z"/></svg>',
  prev: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M21.5 6.2v11.6a.8.8 0 0 1-1.25.66L12 12.66v5.14a.8.8 0 0 1-1.25.66l-8.6-5.8a.8.8 0 0 1 0-1.32l8.6-5.8A.8.8 0 0 1 12 6.2v5.14l8.25-5.8a.8.8 0 0 1 1.25.66z"/></svg>',
  shuffle: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7h3.5c4.5 0 6.5 10 11 10H21M3 17h3.5c1.7 0 3-1.3 4.1-3M13.4 10c1.1-1.7 2.4-3 4.1-3H21"/><path d="m18 4 3 3-3 3M18 14l3 3-3 3"/></svg>',
  info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.5"/></svg>',
};
const ACCENTS = { blue: "#0a84ff", purple: "#bf5af2", pink: "#ff375f", orange: "#ff9f0a", green: "#30d158", graphite: "#98989d" };

$("openApp").innerHTML = ICON.gear;
$("cFocus").querySelector(".disc").innerHTML = ICON.moon;
$("cLock").querySelector(".disc").innerHTML = ICON.lock;
$("cSleep").querySelector(".disc").innerHTML = ICON.power;
$("clipGlass").innerHTML = ICON.search;
$("openGlass").innerHTML = ICON.search;

// ------------------------------------------------------------------ shape

let mode = "rest";        // rest | live | activity | open
let pinned = false;       // opened with the shortcut or by typing: stays until dismissed
let hovering = false;
let hoverTimer = null, leaveTimer = null, shrinkTimer = null;
let snap = null;
// What the island fits around (from the app): a MacBook notch, the Mac
// menu bar, and whether the app follows the pointer for us.
let fit = { notch: null, bar: null, watch: false };
const notchW = () => (fit.notch ? Math.ceil(fit.notch[0]) : 0);
const notchH = () => (fit.notch ? Math.ceil(fit.notch[1]) : 0);
let shown = { w: 0, h: 0, r: 0 };

function restShape() {
  // Around the notch: exactly its size (it disappears into it), with ears
  // while music plays.
  if (fit.notch) return earsWanted() ? { w: notchW() + 2 * 44, h: notchH(), r: 13 } : { w: notchW(), h: notchH(), r: 10 };
  // Mac without a notch: a pill inside the menu bar.
  if (fit.bar) return { w: 196, h: Math.max(22, Math.round(fit.bar) - 2), r: Math.round(fit.bar / 2) };
  // Windows, Linux: a thin lip that lets clicks through to tabs underneath.
  return { w: 150, h: 5, r: 3 };
}
function shapeFor(m) {
  if (m === "live") return fit.notch ? { w: notchW() + 2 * 112, h: notchH(), r: 14 } : { w: 320, h: 36, r: 18 };
  if (m === "activity") return { w: 430, h: 76 + notchH(), r: 32 };
  if (m === "open") return { w: 520, h: Math.ceil($("panel").offsetHeight), r: 30 };
  return restShape();
}

function applyFit(f) {
  if (!f) return;
  const changed = JSON.stringify(f) !== JSON.stringify(fit);
  fit = f;
  document.body.classList.toggle("notch", !!fit.notch);
  document.body.classList.toggle("lip", !fit.notch && !fit.bar);
  document.documentElement.style.setProperty("--notch-h", notchH() + "px");
  if (changed) setMode(mode, true);
}

async function setMode(next, force) {
  const to = shapeFor(next);
  if (next === mode && !force && to.w === shown.w && to.h === shown.h) return;
  mode = next;
  clearTimeout(shrinkTimer);
  const grow = to.w >= shown.w && to.h >= shown.h;
  if (grow) {
    // Make room first (the window is transparent), then morph.
    try { await invoke("island_size", { w: to.w, h: to.h, vw: to.w, vh: to.h }); } catch (_) {}
  } else {
    // Clicks outside the new shape go through straight away.
    invoke("island_size", { w: 0, h: 0, vw: to.w, vh: to.h }).catch(() => {});
  }
  if (mode !== next) return;
  shown = to;
  for (const c of ["open", "activity", "live"]) {
    document.body.classList.toggle(c, next === c);
    island.classList.toggle(c, next === c);
  }
  island.style.setProperty("--w", to.w + "px");
  island.style.setProperty("--h", to.h + "px");
  island.style.setProperty("--r", to.r + "px");
  if (!grow) {
    // Shrink the window once the morph has finished.
    shrinkTimer = setTimeout(() => { if (mode === next) invoke("island_size", { w: to.w, h: to.h, vw: to.w, vh: to.h }).catch(() => {}); }, 480);
  }
}

function open(byShortcut, which) {
  pinned = !!byShortcut;
  clearTimeout(actTimer);
  if (which) setTab(which);
  render();
  if (tab !== "home") refreshTools();
  setMode("open");
  if (byShortcut) focusSearch();
}
function close() {
  pinned = false;
  dropping = null;
  settle();
  setTimeout(() => { if (mode !== "open" && tab === "drop") setTab(lastTab); }, 500);
}
// Back to whatever the island should show when nothing is open.
function settle() {
  if (mode === "open" || mode === "activity") mode = "rest";
  if (queue.length) { nextActivity(); return; }
  const l = liveNow();
  setMode(l ? "live" : "rest");
  if (l) drawLive(l);
}

// ------------------------------------------------------------------ tabs

const TABS = ["home", "clips", "shelf", "open"];
let tab = "home", lastTab = "home";
function setTab(t) {
  if (t !== "drop") lastTab = t;
  tab = t;
  for (const v of document.querySelectorAll(".view")) v.classList.toggle("on", v.dataset.view === t);
  const i = TABS.indexOf(t);
  document.body.classList.toggle("dropping", t === "drop");
  for (const b of $("tabs").querySelectorAll("button")) b.setAttribute("aria-selected", String(b.dataset.tab === t));
  if (i >= 0) $("segThumb").style.transform = `translateX(${i * 100}%)`;
  sel = 0;
  if (t === "clips") renderClips();
  if (t === "shelf") renderShelf();
  if (t === "open") renderApps();
  if (mode === "open") setMode("open");
}
for (const b of $("tabs").querySelectorAll("button")) {
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    setTab(b.dataset.tab);
    if (tab !== "home") refreshTools();
  });
}
function focusSearch() {
  const input = tab === "clips" ? $("clipQuery") : tab === "open" ? $("appQuery") : null;
  if (input) setTimeout(() => { input.focus(); input.select(); }, 60);
}
// Typing in the island keeps it open until it's dismissed.
for (const id of ["clipQuery", "appQuery"]) $(id).addEventListener("focus", () => { pinned = true; });

// Hover: the app watches the pointer (it also makes clicks pass through
// everywhere else); on Wayland the page's own mouse events do.
function onHover(on) {
  hovering = on;
  clearTimeout(hoverTimer);
  clearTimeout(leaveTimer);
  if (on) {
    if (mode !== "open") hoverTimer = setTimeout(() => { if (hovering) open(false); }, mode === "activity" ? 350 : 90);
  } else if (mode === "open" && !pinned && !dropping) {
    leaveTimer = setTimeout(close, 300);
  }
}
if (T) T.event.listen("hover", (e) => { if (fit.watch) onHover(!!e.payload); });
island.addEventListener("mouseenter", () => { if (!fit.watch) onHover(true); });
island.addEventListener("mouseleave", () => { if (!fit.watch) onHover(false); });
island.addEventListener("click", (e) => {
  if (mode === "rest" || mode === "live") open(false);
  else if (mode === "activity" && !e.target.closest(".chip")) open(false);
});
// Keys while it's open: Escape closes; H C S O switch tabs (when not typing).
window.addEventListener("keydown", (e) => {
  if (mode !== "open") return;
  if (e.key === "Escape") { close(); return; }
  if (e.target.tagName === "INPUT" || e.ctrlKey || e.metaKey || e.altKey) return;
  const t = { h: "home", c: "clips", v: "clips", s: "shelf", o: "open" }[e.key.toLowerCase()];
  if (t) { e.preventDefault(); pinned = true; setTab(t); if (t !== "home") refreshTools(); focusSearch(); }
});
window.addEventListener("blur", () => { if (mode === "open" && pinned) close(); });
// Opened with a shortcut: type straight into the search once the window has focus.
window.addEventListener("focus", () => { if (mode === "open" && pinned && document.activeElement === document.body) focusSearch(); });
if (T) T.event.listen("toggle", () => (mode === "open" ? close() : open(true, "home")));
// The tray's Clipboard History and Open an App.
if (T) T.event.listen("tab", (e) => (mode === "open" && tab === e.payload ? close() : open(true, e.payload)));

// ------------------------------------------------------------------ notifications (cards)

const queue = [];
let actTimer = null;
let shownNotes = new Set();

const BADGE = {
  battery: ICON.battery, focus: ICON.moon, files: ICON.files, info: ICON.info, pin: ICON.pin, copied: ICON.check, media: ICON.note,
};
function nextActivity() {
  if (mode === "open" || !queue.length) return;
  const a = queue.shift();
  const kind = a.icon || "info";
  $("actBadge").className = "badge " + kind;
  $("actBadge").innerHTML = BADGE[kind] || ICON.info;
  $("actTitle").textContent = a.title;
  $("actBody").textContent = a.body || "";
  $("actBody").hidden = !a.body;
  const btns = $("actButtons");
  btns.replaceChildren();
  for (const act of a.actions || []) {
    const b = document.createElement("button");
    b.className = "chip" + (btns.children.length ? "" : " primary");
    b.textContent = act.label;
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      invoke("note_action", { kind: act.kind, target: act.target }).catch(() => {});
      clearTimeout(actTimer);
      mode = "rest";
      setTimeout(settle, 60);
    });
    btns.append(b);
  }
  // A new card replays its entrance.
  const act = $("act");
  act.style.animation = "none";
  void act.offsetWidth;
  act.style.animation = "";
  setMode("activity", true);
  const stay = (a.actions || []).length ? 7000 : a.short ? 1900 : 4200;
  const until = Date.now() + 15000;
  clearTimeout(actTimer);
  actTimer = setTimeout(function done() {
    if (mode !== "activity") return;
    // Held while the pointer is on it (never forever).
    if (hovering && Date.now() < until) { actTimer = setTimeout(done, 800); return; }
    mode = "rest";
    settle();
  }, stay);
}

function iconForNote(n) {
  const t = (n.title || "").toLowerCase();
  if (t.includes("battery") || / is at \d+%/.test(n.title || "")) return "battery";
  if (t.includes("focus")) return "focus";
  if (t.includes("file") || t.includes("received") || t.includes("copied")) return "files";
  return "info";
}

// ------------------------------------------------------------------ live pills

// A transfer in flight, one that just finished, the song (briefly after it
// changes, or the whole time around a notch), Focus switching.
let liveFlash = null;     // { kind, until, ... } something shown for a moment
let finished = [];        // transfers that just ended: { label, until, error }
let seenTransfers = new Map();
let lastTrack = "";
let lastFocus = null;

function mediaNow() {
  const m = (snap && snap.media) || [];
  return m.length ? m[0] : null;
}
function earsWanted() {
  const m = mediaNow();
  return !!(fit.notch && m && m.now.playing);
}
function liveNow() {
  const tr = ((snap && snap.transfers) || []).filter((t) => !t.finished);
  if (tr.length) return { kind: "transfer", list: tr };
  finished = finished.filter((f) => f.until > Date.now());
  if (finished.length) return { kind: "done", item: finished[finished.length - 1] };
  if (liveFlash && liveFlash.until > Date.now()) return liveFlash;
  liveFlash = null;
  return null;
}

function ringSvg(p) {
  const c = 2 * Math.PI * 9;
  const off = p == null ? 0 : c * (1 - Math.max(0, Math.min(1, p)));
  return `<svg class="ring${p == null ? " spin" : ""}" viewBox="0 0 22 22"><circle class="track" cx="11" cy="11" r="9"/><circle class="val" cx="11" cy="11" r="9" stroke-dasharray="${c}" stroke-dashoffset="${off}"/></svg>`;
}
const TICK = '<span class="tick"><svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg></span>';
function waveHtml(on) { return `<span class="wave${on ? " on" : ""}"><i></i><i></i><i></i><i></i><i></i><i></i></span>`; }
function esc(s) { const d = document.createElement("span"); d.textContent = s; return d.innerHTML; }
function artStyle(url) { return url ? `background-image:url("${url.replace(/"/g, "%22")}")` : ""; }

let liveKey = "";
function drawLive(l) {
  let left = "", mid = "", right = "";
  if (l.kind === "transfer") {
    const t = l.list[0];
    const done = l.list.reduce((a, x) => a + x.done, 0), total = l.list.reduce((a, x) => a + x.total, 0);
    left = `<span class="c-ico files">${ICON.files}</span>`;
    const label = l.list.length > 1 ? `${l.list.length} transfers` : t.label;
    mid = `${esc(label)}<small>${t.incoming ? "from" : "to"} ${esc(t.peer)}</small>`;
    right = ringSvg(total ? done / total : null);
  } else if (l.kind === "done") {
    left = `<span class="c-ico files">${ICON.files}</span>`;
    mid = l.item.error ? `${esc(l.item.label)}<small>didn't arrive</small>` : `${esc(l.item.label)}<small>${l.item.incoming ? "received" : "sent"}</small>`;
    right = l.item.error ? `<span class="c-tag low">Failed</span>` : TICK;
  } else if (l.kind === "media") {
    const m = l.m.now;
    left = `<span class="c-art" style='${artStyle(m.art)}'></span>`;
    mid = `${esc(m.title)}<small>${esc(m.artist || m.app)}</small>`;
    right = waveHtml(m.playing);
  } else if (l.kind === "focus") {
    left = `<span class="c-ico focus">${ICON.moon}</span>`;
    mid = "Focus";
    right = `<span class="c-tag${l.on ? " on" : ""}">${l.on ? "On" : "Off"}</span>`;
  } else if (l.kind === "copied") {
    left = `<span class="c-ico copied">${ICON.text}</span>`;
    mid = `Copied<small>${esc(l.label)}</small>`;
    right = TICK;
  } else if (l.kind === "opening") {
    left = `<span class="c-app" style="background:${colorFor(l.label)}">${esc(l.label.trim().charAt(0).toUpperCase())}</span>`;
    mid = `${esc(l.label)}<small>Opening${tools && l.on !== tools.me ? " on " + esc(l.on) : ""}</small>`;
    right = ringSvg(null);
  } else if (l.kind === "sending") {
    left = `<span class="c-ico files">${ICON.files}</span>`;
    mid = `${esc(l.label)}<small>${esc(l.to)}</small>`;
    right = ringSvg(null);
  }
  // Only redraw what changed, so rings and bars animate smoothly.
  const key = l.kind + left + mid + (l.kind === "transfer" ? "" : right);
  if (key !== liveKey) {
    liveKey = key;
    $("cLeft").innerHTML = left;
    $("cMid").innerHTML = mid;
    $("cRight").innerHTML = right;
  } else if (l.kind === "transfer") {
    const v = $("cRight").querySelector(".val");
    const ring = $("cRight").querySelector(".ring");
    const total = l.list.reduce((a, x) => a + x.total, 0), done = l.list.reduce((a, x) => a + x.done, 0);
    if (v && total) {
      ring.classList.remove("spin");
      const c = 2 * Math.PI * 9;
      v.setAttribute("stroke-dashoffset", String(c * (1 - done / total)));
    }
  }
}

// Around a notch, the resting ears show the song.
function drawEars() {
  const m = mediaNow();
  if (!fit.notch) return;
  const on = earsWanted();
  $("hints").innerHTML = on ? waveHtml(true) : "";
  $("dot").style.cssText = on ? `width:20px;height:20px;border-radius:6px;box-shadow:none;background:#333 center/cover;${artStyle(m.now.art)}` : "";
}

function updateLive(s) {
  // Transfers that just ended get a tick for a moment.
  const now = new Set();
  for (const t of s.transfers || []) {
    now.add(t.offer);
    seenTransfers.set(t.offer, t);
  }
  for (const [id, t] of seenTransfers) {
    if (!now.has(id)) {
      seenTransfers.delete(id);
      finished.push({ label: t.label, incoming: t.incoming, error: null, until: Date.now() + 1800 });
    }
  }
  // The song changed, or started/stopped.
  const m = mediaNow();
  const key = m ? `${m.name}|${m.now.title}|${m.now.playing}` : "";
  if (key !== lastTrack) {
    if (m && lastTrack !== "" && !fit.notch) liveFlash = { kind: "media", m, until: Date.now() + 3500 };
    else if (m && lastTrack === "" && m.now.playing && !fit.notch) liveFlash = { kind: "media", m, until: Date.now() + 3500 };
    lastTrack = key;
  }
  if (lastFocus !== null && lastFocus !== !!s.focus) liveFlash = { kind: "focus", on: !!s.focus, until: Date.now() + 2200 };
  lastFocus = !!s.focus;
  drawEars();
  if (mode === "open" || mode === "activity") return;
  const l = liveNow();
  if (l) {
    drawLive(l);
    if (mode !== "live") setMode("live");
  } else if (mode === "live") {
    setMode("rest");
  } else {
    setMode("rest");
  }
}

// ------------------------------------------------------------------ content

function fmtGB(bytes) {
  const gb = bytes / 1e9;
  if (gb >= 1000) return `${(gb / 1000).toFixed(gb >= 10000 ? 0 : 1)} TB`;
  return gb >= 100 ? `${Math.round(gb)} GB` : gb >= 10 ? `${gb.toFixed(0)} GB` : `${gb.toFixed(1)} GB`;
}

function batteryEl(b) {
  const [pct, charging] = b;
  const wrap = document.createElement("span");
  wrap.className = "batt" + (charging ? " charging" : pct <= 10 ? " crit" : pct <= 20 ? " low" : "");
  wrap.innerHTML = `<span class="cell"><span class="fill" style="width:${Math.max(6, pct)}%"></span></span>`;
  const t = document.createElement("span");
  t.textContent = `${pct}%`;
  wrap.append(t);
  if (charging) {
    const bolt = document.createElement("span");
    bolt.innerHTML = ICON.bolt;
    bolt.firstChild.setAttribute("width", "11");
    bolt.firstChild.setAttribute("height", "11");
    bolt.style.color = "var(--good)";
    wrap.append(bolt);
  }
  return wrap;
}

function colorFor(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hues = [211, 262, 340, 28, 145, 190, 8, 48];
  const hue = hues[h % hues.length];
  return `linear-gradient(145deg, hsl(${hue}, 80%, 60%), hsl(${hue + 18}, 72%, 45%))`;
}

let lastPanelKey = "";
function render() {
  const s = snap;
  if (!s) return;
  document.documentElement.style.setProperty("--accent", ACCENTS[s.accent] || ACCENTS.blue);
  document.body.classList.toggle("attached", !!s.attached);
  document.body.classList.toggle("floating", !s.attached);

  // At rest: where the pointer is, and small live hints.
  const where = s.active || s.me;
  $("dot").classList.toggle("off", !s.running);
  $("restName").textContent = s.running ? (where === s.me ? "This computer" : where) : "OpenHop is off";
  $("restSub").textContent = "";
  const hints = [];
  if (s.focus) hints.push(`<span class="hint-focus" title="Focus is on">${ICON.moon}</span>`);
  if ((s.computers || []).some((c) => c.status.battery && !c.status.battery[1] && c.status.battery[0] <= 20)) hints.push(`<span class="hint-batt" title="A computer is running low">${ICON.battery}</span>`);
  const tr = (s.transfers || [])[0];
  if (tr) {
    const p = tr.total ? tr.done / tr.total : 0;
    const c = 2 * Math.PI * 6;
    hints.push(`<svg class="ring-progress" viewBox="0 0 15 15"><circle cx="7.5" cy="7.5" r="6" stroke="rgba(255,255,255,.2)"/><circle cx="7.5" cy="7.5" r="6" stroke="var(--accent)" stroke-dasharray="${c}" stroke-dashoffset="${c * (1 - p)}" stroke-linecap="round"/></svg>`);
  }
  $("hints").innerHTML = hints.join("");

  // Open: the Control Center.
  $("headSub").textContent = s.running ? s.message : "OpenHop is off. Open OpenHop to turn it on.";
  $("cFocus").classList.toggle("on", !!s.focus);
  $("cFocus").querySelector(".lbl").textContent = s.focus ? "Focus on" : "Focus";
  $("cFocus").title = s.focus ? "Notifications are silenced on every computer" : "Silence notifications on every computer";

  const key = JSON.stringify([s.computers, s.windows, s.active, s.me]);
  if (key !== lastPanelKey) {
    lastPanelKey = key;
    const pcs = $("pcs");
    pcs.replaceChildren();
    for (const c of s.computers || []) {
      const st = c.status || {};
      const isHere = (s.active || s.me) === c.name;
      const card = document.createElement(c.this || isHere ? "div" : "button");
      card.className = "pc" + (isHere ? " here" : "") + (c.this || isHere ? "" : " go");
      if (!c.this && !isHere) {
        card.title = `Move the pointer to ${c.name}`;
        card.addEventListener("click", (e) => { e.stopPropagation(); close(); invoke("go_to", { name: c.name }).catch(() => {}); });
      }
      const top = document.createElement("div");
      top.className = "top";
      top.innerHTML = st.battery ? ICON.laptop : ICON.desktop;
      const nm = document.createElement("span");
      nm.className = "name";
      nm.textContent = c.this ? `${c.name} (this)` : c.name;
      top.append(nm);
      card.append(top);
      const line = document.createElement("div");
      line.className = "state";
      if (isHere) line.innerHTML = '<span class="pointer">Pointer here</span>';
      else if (st.locked) line.textContent = "Locked";
      else if (st.fullscreen) line.textContent = "Full screen";
      else line.textContent = "Ready";
      if (st.battery) { line.append(document.createTextNode(" ")); line.append(batteryEl(st.battery)); }
      card.append(line);
      if (st.disk) {
        const [free, total] = st.disk;
        const d = document.createElement("div");
        d.className = "disk";
        d.innerHTML = `<div class="bar"><i style="width:${Math.round((1 - free / Math.max(1, total)) * 100)}%"></i></div>`;
        const sm = document.createElement("small");
        sm.textContent = `${fmtGB(free)} free`;
        sm.title = `${fmtGB(free)} free of ${fmtGB(total)}`;
        d.append(sm);
        card.append(d);
      }
      pcs.append(card);
    }
    if (!(s.computers || []).length) {
      const e = document.createElement("div");
      e.className = "empty";
      e.textContent = "Your computers appear here once OpenHop is on.";
      pcs.append(e);
    }
    const wins = $("wins");
    wins.replaceChildren();
    const others = (s.windows || []).filter((g) => g.name !== s.me);
    for (const g of others) {
      for (const w of g.windows.slice(0, 12)) {
        const b = document.createElement("button");
        b.className = "win";
        b.title = `${w.title} on ${g.name}`;
        const app = document.createElement("span");
        app.className = "app";
        app.style.background = colorFor(w.app || w.title);
        app.textContent = (w.app || w.title || "?").trim().charAt(0).toUpperCase();
        const n = document.createElement("span");
        n.className = "n";
        n.textContent = w.title;
        const o = document.createElement("span");
        o.className = "o";
        o.textContent = g.name;
        b.append(app, n, o);
        b.addEventListener("click", async () => {
          try { await invoke("open_window", { origin: g.name, window: String(w.id) }); } catch (_) {}
          close();
        });
        wins.append(b);
      }
    }
    $("winsTitle").hidden = !others.length;
    if (!others.length) {
      const e = document.createElement("div");
      e.className = "empty";
      e.textContent = s.connected ? "No windows open on your other computers." : "";
      wins.append(e);
    }
    if (mode === "open") setMode("open");
  }
}

// ------------------------------------------------------------------ controls

$("openApp").addEventListener("click", (e) => { e.stopPropagation(); invoke("island_open_app"); close(); });
$("cFocus").addEventListener("click", (e) => {
  e.stopPropagation();
  const on = !(snap && snap.focus);
  if (snap) snap.focus = on;
  render();
  invoke("island_focus", { on }).catch(() => {});
});
$("cLock").addEventListener("click", (e) => { e.stopPropagation(); close(); invoke("island_lock_all").catch(() => {}); });
// Sleep is a big step: press once to arm, again to confirm.
let sleepArmed = null;
$("cSleep").addEventListener("click", (e) => {
  e.stopPropagation();
  const c = $("cSleep");
  if (sleepArmed) {
    clearTimeout(sleepArmed);
    sleepArmed = null;
    c.classList.remove("armed");
    close();
    invoke("island_sleep_all").catch(() => {});
    return;
  }
  c.classList.add("armed");
  c.querySelector(".lbl").textContent = "Click to confirm";
  sleepArmed = setTimeout(() => { sleepArmed = null; c.classList.remove("armed"); c.querySelector(".lbl").textContent = "Sleep all"; }, 2500);
});

// ------------------------------------------------------------------ clipboard, shelf, open

let tools = null;
let sel = 0;
let appWhere = "";        // "" = every computer
let lastToolsKey = "";

async function refreshTools() {
  try {
    tools = await invoke("tools_state");
  } catch (_) { return; }
  const key = JSON.stringify([tools.clips, tools.shelf, tools.apps.map((a) => [a.name, a.apps.length]), tools.tasks]);
  if (key === lastToolsKey) return;
  lastToolsKey = key;
  if (tab === "clips") renderClips();
  if (tab === "shelf") renderShelf();
  if (tab === "open") renderApps();
  if (mode === "open") setMode("open");
}

function ago(at) {
  const s = Math.max(0, Date.now() / 1000 - at);
  if (s < 50) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(at * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
function fromName(n) { return tools && n === tools.me ? "this computer" : n; }

function button(icon, title, fn, on) {
  const b = document.createElement("button");
  b.innerHTML = icon;
  b.title = title;
  b.setAttribute("aria-label", title);
  if (on) b.classList.add("on");
  b.addEventListener("click", (e) => { e.stopPropagation(); fn(); });
  return b;
}

function empty(text) {
  const e = document.createElement("div");
  e.className = "empty";
  e.style.padding = "18px 4px";
  e.textContent = text;
  return e;
}

function moveSel(list, d) {
  const rows = list.querySelectorAll(".row");
  if (!rows.length) return;
  sel = (sel + d + rows.length) % rows.length;
  rows.forEach((r, i) => r.classList.toggle("sel", i === sel));
  rows[sel].scrollIntoView({ block: "nearest" });
}

function clipMatches() {
  const q = $("clipQuery").value.trim().toLowerCase();
  const all = (tools && tools.clips) || [];
  const v = q ? all.filter((c) => c.text.toLowerCase().includes(q) || c.from.toLowerCase().includes(q)) : all;
  return [...v.filter((c) => c.pinned), ...v.filter((c) => !c.pinned)];
}

function useClip(c, row) {
  invoke("clip_use", { id: c.id }).catch(() => {});
  if (row) row.classList.add("done");
  liveFlash = { kind: "copied", label: c.kind === "text" ? c.text.replace(/\s+/g, " ").slice(0, 60) : c.text, until: Date.now() + 1500 };
  setTimeout(close, 200);
}

function renderClips() {
  const list = $("clipList");
  list.replaceChildren();
  const items = clipMatches();
  if (!items.length) {
    list.append(empty(tools && tools.clips.length ? "Nothing matches." : "Things you copy on any of your computers show up here."));
    return;
  }
  sel = Math.min(sel, items.length - 1);
  items.forEach((c, i) => {
    const row = document.createElement("div");
    row.className = "row" + (i === sel ? " sel" : "");
    row.tabIndex = -1;
    const lead = document.createElement("span");
    lead.className = "lead";
    if (c.thumb) {
      const img = document.createElement("img");
      img.src = c.thumb;
      img.alt = "";
      lead.append(img);
    } else lead.innerHTML = c.kind === "files" ? ICON.files : c.kind === "image" ? ICON.image : ICON.text;
    const body = document.createElement("span");
    body.className = "body";
    const main = document.createElement("div");
    main.className = "main";
    main.textContent = c.text;
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = `From ${fromName(c.from)}, ${ago(c.at)}`;
    body.append(main, meta);
    const tail = document.createElement("span");
    tail.className = "tail";
    tail.append(
      button(ICON.pinned, c.pinned ? "Unpin" : "Pin (kept until you unpin it)", () => { invoke("clip_pin", { id: c.id, pinned: !c.pinned }).then(refreshTools); }, c.pinned),
      button(ICON.trash, "Remove from history", () => { invoke("clip_forget", { id: c.id }).then(refreshTools); }),
    );
    row.append(lead, body, tail);
    row.addEventListener("click", (e) => { e.stopPropagation(); useClip(c, row); });
    list.append(row);
  });
}
$("clipQuery").addEventListener("input", () => { sel = 0; renderClips(); setMode("open"); });
$("clipQuery").addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); moveSel($("clipList"), e.key === "ArrowDown" ? 1 : -1); }
  else if (e.key === "Enter") { const c = clipMatches()[sel]; if (c) useClip(c, $("clipList").querySelectorAll(".row")[sel]); }
});

const EXT_COLORS = { pdf: 4, doc: 0, docx: 0, txt: 6, md: 6, png: 2, jpg: 2, jpeg: 2, gif: 2, heic: 2, mp4: 3, mov: 3, mp3: 5, wav: 5, zip: 1, rar: 1, "7z": 1, xls: 4, xlsx: 4, csv: 4, ppt: 3, pptx: 3 };
function docColor(label, isFolder) {
  if (isFolder) return "linear-gradient(160deg,#64b5ff,#2f7de1)";
  const ext = (label.split(".").pop() || "").toLowerCase();
  const hues = [211, 262, 145, 8, 145, 300, 220];
  const h = ext in EXT_COLORS ? hues[EXT_COLORS[ext]] : 220;
  return `linear-gradient(160deg, hsl(${h},70%,62%), hsl(${h + 14},62%,46%))`;
}
function fmtSize(b) {
  if (b < 1e3) return `${b} B`;
  if (b < 1e6) return `${Math.round(b / 1e3)} KB`;
  if (b < 1e9) return `${(b / 1e6).toFixed(b < 1e7 ? 1 : 0)} MB`;
  return `${(b / 1e9).toFixed(1)} GB`;
}

function renderShelf() {
  const grid = $("shelfGrid");
  grid.replaceChildren();
  const items = (tools && tools.shelf) || [];
  if (!items.length) {
    const b = document.createElement("div");
    b.className = "blank";
    b.innerHTML = ICON.tray;
    b.append(document.createTextNode("The shelf is empty. Drag files onto the island to put them here."));
    grid.append(b);
    return;
  }
  for (const { origin, item } of items) {
    const mine = tools && origin === tools.me;
    const t = document.createElement("button");
    t.className = "tile";
    t.title = mine ? `${item.label} (on this computer)` : `Copy ${item.label} to this computer`;
    const folder = item.files.length > 1 || (item.files[0] && item.files[0].path.includes("/"));
    const doc = document.createElement("span");
    doc.className = "doc";
    doc.style.background = docColor(item.label, folder);
    const ext = (item.label.includes(".") && !folder) ? item.label.split(".").pop().slice(0, 4).toUpperCase() : "";
    doc.textContent = ext;
    if (folder) { doc.innerHTML = ICON.files; doc.style.borderRadius = "8px"; }
    const n = document.createElement("span");
    n.className = "n";
    n.textContent = item.label;
    const o = document.createElement("span");
    o.className = "o";
    o.textContent = `${fmtSize(item.size)}, ${mine ? "this computer" : origin}`;
    const x = document.createElement("span");
    x.className = "x";
    x.innerHTML = ICON.x;
    x.title = "Take off the shelf";
    x.addEventListener("click", (e) => { e.stopPropagation(); t.style.transform = "scale(.8)"; t.style.opacity = "0"; t.style.transition = "all .25s"; invoke("shelf_remove", { origin, id: item.id }).then(() => setTimeout(refreshTools, 250)); });
    t.append(x, doc, n, o);
    t.addEventListener("click", (e) => {
      e.stopPropagation();
      if (mine) return;
      t.classList.add("busy");
      invoke("shelf_take", { origin, id: item.id }).catch(() => {});
      liveFlash = { kind: "sending", label: item.label, to: `from ${origin} to the clipboard here`, until: Date.now() + 4000 };
      setTimeout(close, 500);
    });
    grid.append(t);
  }
}

// The Open tab: like a task manager. What's open (in the dock or taskbar),
// what's running in the background, then every app to open, on every computer.
const quitting = new Map();   // "computer|app" → { at, pids, force }

function fmtMem(b) {
  if (!b) return "";
  if (b < 1e9) return `${Math.max(1, Math.round(b / 1e6))} MB`;
  return `${(b / 1e9).toFixed(b < 1e10 ? 1 : 0)} GB`;
}
function whereName(n) { return tools && n === tools.me ? "This computer" : n; }

function openItems() {
  const q = $("appQuery").value.trim().toLowerCase();
  const score = (name) => {
    const n = name.toLowerCase();
    if (!q) return 1;
    if (n.startsWith(q)) return 3;
    if (n.split(/[\s\-_.]+/).some((w) => w.startsWith(q))) return 2;
    return n.includes(q) ? 1 : 0;
  };
  const open = [], bg = [], apps = [];
  const running = new Set();
  for (const [on, list] of (tools && tools.tasks) || []) {
    if (appWhere && on !== appWhere) continue;
    for (const t of list) {
      running.add(`${on}|${t.app_id || t.name}`);
      const sc = score(t.name);
      if (!sc) continue;
      (t.windows.length ? open : bg).push({ kind: t.windows.length ? "open" : "bg", on, task: t, name: t.name, score: sc });
    }
  }
  for (const g of (tools && tools.apps) || []) {
    if (appWhere && g.name !== appWhere) continue;
    for (const a of g.apps) {
      const sc = score(a.name);
      if (!sc) continue;
      // Running ones are listed above (unless searching: then they're first anyway).
      if (!q && running.has(`${g.name}|${a.id}`)) continue;
      apps.push({ kind: "app", on: g.name, app: a, name: a.name, score: sc });
    }
  }
  const order = (x, y) => y.score - x.score || x.name.localeCompare(y.name);
  if (q) { open.sort(order); bg.sort(order); apps.sort(order); }
  return { open, bg, apps: apps.slice(0, q ? 40 : 200) };
}

function launch(m) {
  invoke("app_launch", { on: m.on, id: m.app.id }).catch(() => {});
  liveFlash = { kind: "opening", label: m.app.name, on: m.on, until: Date.now() + 2600 };
  $("appQuery").value = "";
  close();
}

function quitButton(m) {
  const key = `${m.on}|${m.name}`;
  const q = quitting.get(key);
  const stuck = q && Date.now() - q.at > 4000;
  const b = document.createElement("button");
  b.className = "qbtn" + (stuck ? " force" : "");
  b.innerHTML = stuck ? "Force Quit" : ICON.x;
  b.title = stuck ? `${m.name} isn't closing. Force it to quit (unsaved work is lost).` : `Quit ${m.name}`;
  b.setAttribute("aria-label", b.title);
  let armed = false, timer = null;
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!armed) {
      armed = true;
      b.classList.add("armed");
      b.textContent = stuck ? "Force Quit?" : "Quit";
      timer = setTimeout(() => { armed = false; b.classList.remove("armed"); b.innerHTML = stuck ? "Force Quit" : ICON.x; }, 2600);
      return;
    }
    clearTimeout(timer);
    quitting.set(key, { at: Date.now(), force: stuck });
    invoke("task_quit", { on: m.on, pids: m.task.pids, force: !!stuck }).catch(() => {});
    const row = b.closest(".row");
    if (row) row.classList.add("leaving");
    setTimeout(refreshTools, 900);
  });
  return b;
}

function appRow(m, i) {
  const row = document.createElement("div");
  row.className = "row" + (i === sel ? " sel" : "");
  row.tabIndex = -1;
  const lead = document.createElement("span");
  lead.className = "lead app";
  lead.style.background = colorFor(m.name);
  lead.textContent = m.name.trim().charAt(0).toUpperCase();
  if (m.kind === "open") lead.classList.add("running");
  const body = document.createElement("span");
  body.className = "body";
  const main = document.createElement("div");
  main.className = "main";
  main.textContent = m.name;
  body.append(main);
  if (m.kind !== "app") {
    const meta = document.createElement("div");
    meta.className = "meta";
    const w = m.task.windows.length;
    const parts = [];
    if (w) parts.push(w === 1 ? "1 window" : `${w} windows`);
    else parts.push("No windows");
    if (m.task.memory) parts.push(fmtMem(m.task.memory));
    if (quitting.has(`${m.on}|${m.name}`)) parts.push("Quitting…");
    meta.textContent = parts.join(", ");
    body.append(meta);
  }
  row.append(lead, body);
  const multi = ((tools && tools.tasks) || []).length > 1 || ((tools && tools.apps) || []).length > 1;
  if (multi) {
    const w = document.createElement("span");
    w.className = "where";
    w.textContent = whereName(m.on);
    row.append(w);
  }
  if (m.kind !== "app") {
    const tail = document.createElement("span");
    tail.className = "tail";
    tail.append(quitButton(m));
    row.append(tail);
  }
  row.addEventListener("click", (e) => {
    e.stopPropagation();
    if (m.kind === "app") return launch(m);
    if (m.kind === "open") {
      invoke("task_raise", { on: m.on, window: m.task.windows[0] }).catch(() => {});
      close();
      return;
    }
    // In the background: open its window, the way its own icon would.
    if (m.task.app_id) launch({ on: m.on, app: { id: m.task.app_id, name: m.name } });
  });
  return row;
}

function section(list, title, items, start) {
  if (!items.length) return start;
  const h = document.createElement("div");
  h.className = "sect";
  h.textContent = title;
  list.append(h);
  items.forEach((m, k) => list.append(appRow(m, start + k)));
  return start + items.length;
}

function renderApps() {
  const where = $("appWhere");
  where.replaceChildren();
  const names = new Set([...((tools && tools.apps) || []).map((g) => g.name), ...((tools && tools.tasks) || []).map(([n]) => n)]);
  if (names.size > 1) {
    for (const [label, val] of [["Every computer", ""], ...[...names].map((n) => [whereName(n), n])]) {
      const b = document.createElement("button");
      b.textContent = label;
      b.classList.toggle("on", appWhere === val);
      b.addEventListener("click", (e) => { e.stopPropagation(); appWhere = val; sel = 0; renderApps(); setMode("open"); });
      where.append(b);
    }
  }
  where.hidden = names.size <= 1;
  // Forget quits that finished.
  for (const [k, q] of quitting) {
    const [on, name] = k.split("|");
    const still = ((tools && tools.tasks) || []).some(([n, l]) => n === on && l.some((t) => t.name === name));
    if (!still || Date.now() - q.at > 30000) quitting.delete(k);
  }
  const list = $("appList");
  const scroll = list.scrollTop;
  list.replaceChildren();
  const { open, bg, apps } = openItems();
  const total = open.length + bg.length + apps.length;
  if (!total) {
    list.append(empty(names.size ? "Nothing by that name." : "Your computers' apps show up here once OpenHop is on."));
    return;
  }
  sel = Math.min(sel, total - 1);
  let i = section(list, "Open", open, 0);
  i = section(list, "In the background", bg, i);
  section(list, $("appQuery").value.trim() ? "Apps" : "All apps", apps, i);
  list.scrollTop = scroll;
}
$("appQuery").addEventListener("input", () => { sel = 0; renderApps(); setMode("open"); });
$("appQuery").addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); moveSel($("appList"), e.key === "ArrowDown" ? 1 : -1); }
  else if (e.key === "Enter") { const r = $("appList").querySelectorAll(".row")[sel]; if (r) r.click(); }
});

// ------------------------------------------------------------------ drop files on the island

let dropping = null;      // { paths, hot }
function dropTargets() {
  const names = ((tools && tools.computers) || []);
  const v = names.map((n) => ({ to: n, label: n, icon: ICON.laptop }));
  if (names.length > 1) v.push({ to: "*", label: "All computers", icon: ICON.all });
  v.push({ to: "", label: "Shelf", icon: ICON.tray });
  return v;
}
function renderDrop() {
  const box = $("dropTargets");
  box.replaceChildren();
  for (const t of dropTargets()) {
    const el = document.createElement("div");
    el.className = "target";
    el.dataset.to = t.to;
    el.innerHTML = t.icon;
    const l = document.createElement("span");
    l.textContent = t.label;
    el.append(l);
    box.append(el);
  }
}
function hitTarget(pos) {
  if (!pos) return null;
  const x = pos.x / devicePixelRatio, y = pos.y / devicePixelRatio;
  let hot = null;
  for (const el of $("dropTargets").children) {
    const r = el.getBoundingClientRect();
    const inside = x >= r.left - 5 && x <= r.right + 5 && y >= r.top - 5 && y <= r.bottom + 5;
    el.classList.toggle("hot", inside);
    if (inside) hot = el.dataset.to;
  }
  return hot;
}
function itemsLabel(paths) {
  if (paths.length !== 1) return `${paths.length} items`;
  return paths[0].split(/[\\/]/).pop();
}
async function onDrag(e) {
  const p = e.payload || {};
  if (p.type === "enter") {
    dropping = { paths: p.paths || [], hot: null };
    $("dropTitle").textContent = dropping.paths.length ? `Send ${itemsLabel(dropping.paths)}` : "Send";
    await refreshTools();
    clearTimeout(leaveTimer);
    clearTimeout(actTimer);
    setTab("drop");
    renderDrop();
    pinned = true;
    setMode("open");
  } else if (p.type === "over") {
    if (dropping) dropping.hot = hitTarget(p.position);
  } else if (p.type === "drop") {
    const paths = p.paths && p.paths.length ? p.paths : dropping ? dropping.paths : [];
    // Dropped elsewhere on the island: the shelf.
    const to = hitTarget(p.position) ?? (dropping && dropping.hot) ?? "";
    dropping = null;
    if (paths.length) {
      if (to === "") {
        invoke("shelf_put", { paths }).catch(() => {});
        queue.push({ title: "On the shelf", body: `${itemsLabel(paths)} can now be picked up from any computer.`, icon: "files" });
      } else {
        invoke("send_paths", { to, paths }).catch(() => {});
        liveFlash = { kind: "sending", label: itemsLabel(paths), to: to === "*" ? "to all your computers" : `to ${to}`, until: Date.now() + 4000 };
      }
    }
    close();
  } else if (p.type === "leave") {
    dropping = null;
    if (!hovering) close();
  }
}
if (T && T.webview) T.webview.getCurrentWebview().onDragDropEvent(onDrag);

// ------------------------------------------------------------------ data

// ------------------------------------------------------------------ media player

let mediaPick = 0;        // which computer's music, when several play
let shownMedia = null;
function fmtTime(t) {
  if (t == null || !isFinite(t)) return "–:––";
  t = Math.max(0, Math.round(t));
  const h = Math.floor(t / 3600), m = Math.floor((t % 3600) / 60), sec = t % 60;
  return (h ? `${h}:${String(m).padStart(2, "0")}` : `${m}`) + `:${String(sec).padStart(2, "0")}`;
}
function positionOf(n) {
  if (n.position == null) return null;
  const p = n.position + (n.playing ? (Date.now() - n.at) / 1000 : 0);
  return n.duration ? Math.min(p, n.duration) : p;
}
$("pPrev").innerHTML = ICON.prev;
$("pNext").innerHTML = ICON.next;
$("pShuffle").innerHTML = ICON.shuffle;
// A command shows right away (the real state follows a moment later).
function mediaCmd(cmd, at) {
  if (!shownMedia) return;
  const n = shownMedia.now;
  if (cmd === "toggle") { n.position = positionOf(n); n.at = Date.now(); n.playing = !n.playing; }
  if (cmd === "seek") { n.position = at; n.at = Date.now(); }
  if (cmd === "shuffle" && n.shuffle != null) n.shuffle = !n.shuffle;
  drawPlayer();
  invoke("media_cmd", { on: shownMedia.name, cmd, at: at ?? null }).catch(() => {});
}
$("pPlay").addEventListener("click", (e) => { e.stopPropagation(); mediaCmd("toggle"); });
$("pNext").addEventListener("click", (e) => { e.stopPropagation(); mediaCmd("next"); });
$("pPrev").addEventListener("click", (e) => { e.stopPropagation(); mediaCmd("previous"); });
$("pShuffle").addEventListener("click", (e) => { e.stopPropagation(); mediaCmd("shuffle"); });
$("pWhere").addEventListener("click", (e) => { e.stopPropagation(); mediaPick++; renderPlayer(true); });
$("pBar").addEventListener("click", (e) => {
  e.stopPropagation();
  const n = shownMedia && shownMedia.now;
  if (!n || !n.duration) return;
  const r = $("pBar").getBoundingClientRect();
  mediaCmd("seek", n.duration * Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)));
});

// The art's colours tint the player, like the system's own.
function tintFrom(url) {
  if (!url) { $("player").style.setProperty("--tint", "transparent"); return; }
  const img = new Image();
  img.crossOrigin = "anonymous";
  img.onload = () => {
    try {
      const c = document.createElement("canvas");
      c.width = c.height = 8;
      const g = c.getContext("2d");
      g.drawImage(img, 0, 0, 8, 8);
      const d = g.getImageData(0, 0, 8, 8).data;
      let r = 0, gg = 0, b = 0;
      for (let i = 0; i < d.length; i += 4) { r += d[i]; gg += d[i + 1]; b += d[i + 2]; }
      const n = d.length / 4;
      const col = `rgb(${r / n | 0}, ${gg / n | 0}, ${b / n | 0})`;
      $("player").style.setProperty("--tint", `radial-gradient(circle at 20% 30%, ${col}, transparent 60%)`);
      $("player").style.setProperty("--wave", `color-mix(in srgb, ${col} 55%, white)`);
    } catch (_) {}
  };
  img.src = url;
}

let lastArt = null;
function renderPlayer(force) {
  const list = (snap && snap.media) || [];
  const p = $("player");
  if (!list.length) {
    if (!p.hidden) { p.hidden = true; shownMedia = null; if (mode === "open") setMode("open"); }
    return;
  }
  const m = list[mediaPick % list.length];
  // Keep local changes (just pressed play) until the computer reports back.
  const fresh = !shownMedia || force || shownMedia.name !== m.name || m.now.at > (shownMedia.now.at || 0) + 600;
  if (fresh) shownMedia = JSON.parse(JSON.stringify(m));
  const wasHidden = p.hidden;
  p.hidden = false;
  const n = shownMedia.now;
  $("pTitle").textContent = n.title;
  $("pArtist").textContent = n.artist || n.album || n.app;
  if (n.art !== lastArt) {
    lastArt = n.art;
    $("pArt").style.backgroundImage = n.art ? `url("${n.art.replace(/"/g, "%22")}")` : "";
    $("pArt").innerHTML = n.art ? "" : ICON.note;
    tintFrom(n.art);
  }
  const me = snap.me;
  $("pWhere").textContent = (m.name === me ? n.app : `${n.app} on ${m.name}`) + (list.length > 1 ? "  ›" : "");
  $("pWhere").title = list.length > 1 ? "Show what's playing on another computer" : "";
  $("pShuffle").hidden = n.shuffle == null;
  drawPlayer();
  if (wasHidden && mode === "open") setMode("open");
}
function drawPlayer() {
  if (!shownMedia) return;
  const n = shownMedia.now;
  $("pPlay").innerHTML = n.playing ? ICON.pause : ICON.play;
  $("pWave").classList.toggle("on", n.playing);
  $("pShuffle").classList.toggle("on", !!n.shuffle);
  const pos = positionOf(n);
  $("pPos").textContent = fmtTime(pos);
  $("pDur").textContent = n.duration ? "-" + fmtTime(n.duration - (pos || 0)) : "";
  $("pFill").style.width = n.duration && pos != null ? `${(100 * pos) / n.duration}%` : "0%";
  $("pBar").style.visibility = n.duration ? "" : "hidden";
}
// The clock runs between updates.
setInterval(() => { if (mode === "open" && tab === "home") drawPlayer(); }, 500);

async function poll() {
  try {
    snap = await invoke("island_state");
    applyFit(snap.fit);
    for (const n of snap.notes || []) {
      if (shownNotes.has(n.id)) continue;
      shownNotes.add(n.id);
      queue.push({ title: n.title, body: n.body, icon: iconForNote(n), actions: n.actions });
    }
    for (const a of snap.activities || []) queue.push(a);
    render();
    renderPlayer();
    if ((mode === "rest" || mode === "live") && queue.length) nextActivity();
    else updateLive(snap);
  } catch (_) {}
  if (mode === "open" && tab !== "home" && tab !== "drop") refreshTools();
  setTimeout(poll, mode === "open" ? 450 : 900);
}
setTab("home");
applyFit(fit);
poll();

// ------------------------------------------------------------------ browser preview

function demo() {
  const state = {
    running: true, me: "desk", message: "Sharing with 2 computers", active: "macbook", here: false, focus: false, connected: 2,
    attached: true, accent: "blue", dark: true,
    computers: [
      { name: "desk", this: true, status: { battery: null, disk: [412e9, 1000e9], focus: false, locked: false, fullscreen: false } },
      { name: "macbook", this: false, status: { battery: [64, true], disk: [128e9, 512e9], focus: false, locked: false, fullscreen: false } },
      { name: "ubuntu-box", this: false, status: { battery: [14, false], disk: [38e9, 256e9], focus: false, locked: true, fullscreen: false } },
    ],
    windows: [
      { name: "desk", windows: [] },
      { name: "macbook", windows: [{ id: 1, title: "Design review.key", app: "Keynote" }, { id: 2, title: "Spotify", app: "Spotify" }, { id: 3, title: "Safari", app: "Safari" }] },
      { name: "ubuntu-box", windows: [{ id: 4, title: "Terminal", app: "gnome-terminal" }, { id: 5, title: "Files", app: "Nautilus" }] },
    ],
    transfers: location.hash.includes("transfer") ? [{ offer: 1, label: "holiday.mp4", peer: "macbook", incoming: false, done: 40, total: 100 }] : [],
    fit: { notch: location.hash.includes("notch") ? [200, 32] : null, bar: location.hash.includes("notch") ? 32 : null, watch: false },
    media: [{ name: "macbook", now: { app: "Spotify", title: "In the Flat Field", artist: "Bauhaus", album: "In the Flat Field", art: null, playing: true, position: 231, duration: 300, at: Date.now(), shuffle: false } }],
    notes: [], activities: [],
  };
  let first = true;
  return async (cmd, args) => {
    if (cmd === "island_state") {
      const s = structuredClone(state);
      if (first && !location.hash.includes("quiet")) { s.activities = [{ title: "ubuntu-box is at 14%", body: "Plug it in soon.", icon: "battery" }]; first = false; }
      return s;
    }
    if (cmd === "island_focus") state.focus = args.on;
    if (cmd === "tools_state") {
      const now = Date.now() / 1000;
      return {
        me: "desk", computers: ["macbook", "ubuntu-box"],
        clips: [
          { id: 1, kind: "text", text: "https://github.com/maykano-dev/open-hop/releases", thumb: null, from: "macbook", at: now - 40, pinned: true },
          { id: 2, kind: "text", text: "Meeting moved to Thursday 3 pm. Bring the printed slides and the budget sheet.", thumb: null, from: "desk", at: now - 300, pinned: false },
          { id: 3, kind: "files", text: "holiday.mp4, notes.pdf", thumb: null, from: "ubuntu-box", at: now - 4000, pinned: false },
          { id: 4, kind: "image", text: "Image 1280 × 720", thumb: null, from: "macbook", at: now - 9000, pinned: false },
        ],
        shelf: [
          { origin: "macbook", item: { id: 7, label: "Design review.key", size: 48200000, files: [{ path: "Design review.key", size: 1 }] } },
          { origin: "desk", item: { id: 8, label: "invoice-0923.pdf", size: 220000, files: [{ path: "invoice-0923.pdf", size: 1 }] } },
          { origin: "ubuntu-box", item: { id: 9, label: "photos", size: 812000000, files: [{ path: "photos/a.jpg", size: 1 }, { path: "photos/b.jpg", size: 1 }] } },
        ],
        tasks: [
          ["desk", [
            { name: "Firefox", pids: [11], windows: [101, 102, 103], memory: 1.42e9, app_id: "d1" },
            { name: "Visual Studio Code", pids: [12], windows: [104], memory: 812e6, app_id: "d4" },
            { name: "Spotify", pids: [13], windows: [], memory: 296e6, app_id: "d5" },
            { name: "Discord", pids: [14], windows: [], memory: 410e6, app_id: "d6" },
          ]],
          ["macbook", [
            { name: "Keynote", pids: [21], windows: [201], memory: 640e6, app_id: "m1" },
            { name: "Safari", pids: [22], windows: [202, 203], memory: 980e6, app_id: "m2" },
          ]],
        ],
        apps: [
          { name: "desk", apps: ["Calculator", "Firefox", "GIMP", "Terminal", "Visual Studio Code"].map((n, i) => ({ id: "d" + i, name: n })) },
          { name: "macbook", apps: ["Final Cut Pro", "Keynote", "Safari", "Terminal", "Xcode"].map((n, i) => ({ id: "m" + i, name: n })) },
        ],
      };
    }
    return null;
  };
}
