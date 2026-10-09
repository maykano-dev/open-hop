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
  info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.5"/></svg>',
};
const ACCENTS = { blue: "#0a84ff", purple: "#bf5af2", pink: "#ff375f", orange: "#ff9f0a", green: "#30d158", graphite: "#98989d" };

$("openApp").innerHTML = ICON.gear;
$("cFocus").querySelector(".disc").innerHTML = ICON.moon;
$("cFind").querySelector(".disc").innerHTML = ICON.find;
$("cLock").querySelector(".disc").innerHTML = ICON.lock;
$("cSleep").querySelector(".disc").innerHTML = ICON.power;
$("clipGlass").innerHTML = ICON.search;
$("openGlass").innerHTML = ICON.search;

// ------------------------------------------------------------------ shape

let mode = "rest";        // rest | activity | open
let pinned = false;       // opened with the shortcut: stays until dismissed
let hovering = false;
let hoverTimer = null, leaveTimer = null, shrinkTimer = null;
let snap = null;

const REST = { w: 210, h: 34, r: 17 };
function shapeFor(m) {
  if (m === "activity") return { w: 430, h: 76, r: 32 };
  if (m === "open") return { w: 520, h: Math.ceil($("panel").offsetHeight), r: 30 };
  return REST;
}

async function setMode(next) {
  if (next === mode && next !== "open") return;
  const from = shapeFor(mode), to = shapeFor(next);
  mode = next;
  clearTimeout(shrinkTimer);
  const grow = to.w >= from.w && to.h >= from.h;
  if (grow) {
    // Make room first (the window is transparent), then morph.
    try { await invoke("island_size", { w: to.w, h: to.h }); } catch (_) {}
  }
  document.body.classList.toggle("open", next === "open");
  document.body.classList.toggle("activity", next === "activity");
  island.classList.toggle("open", next === "open");
  island.classList.toggle("activity", next === "activity");
  island.style.setProperty("--w", to.w + "px");
  island.style.setProperty("--h", to.h + "px");
  island.style.setProperty("--r", to.r + "px");
  if (!grow) {
    // Shrink the window once the morph has finished.
    shrinkTimer = setTimeout(() => { if (mode === next) invoke("island_size", { w: to.w, h: to.h }).catch(() => {}); }, 480);
  }
}

function open(byShortcut, which) {
  pinned = !!byShortcut;
  if (which) setTab(which);
  render();
  if (tab !== "home") refreshTools();
  setMode("open");
  if (byShortcut) focusSearch();
}
function close() {
  pinned = false;
  dropping = null;
  setMode("rest");
  setTimeout(() => { if (mode === "rest" && tab === "drop") setTab(lastTab); }, 500);
  setTimeout(nextActivity, 520);
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

island.addEventListener("mouseenter", () => {
  hovering = true;
  clearTimeout(leaveTimer);
  if (mode !== "open") hoverTimer = setTimeout(() => open(false), 140);
});
island.addEventListener("mouseleave", () => {
  hovering = false;
  clearTimeout(hoverTimer);
  if (mode === "open" && !pinned) leaveTimer = setTimeout(close, 380);
});
island.addEventListener("click", (e) => {
  if (mode === "rest") open(false);
  else if (mode === "activity" && !e.target.closest(".chip")) { clearTimeout(actTimer); open(false); }
});
window.addEventListener("keydown", (e) => { if (e.key === "Escape" && mode === "open") close(); });
window.addEventListener("blur", () => { if (mode === "open" && pinned) close(); });
// Opened with a shortcut: type straight into the search once the window has focus.
window.addEventListener("focus", () => { if (mode === "open" && pinned && document.activeElement === document.body) focusSearch(); });
if (T) T.event.listen("toggle", () => (mode === "open" ? close() : open(true, "home")));
// Ctrl+Alt+V (clipboard) and Ctrl+Alt+O (open an app).
if (T) T.event.listen("tab", (e) => (mode === "open" && tab === e.payload ? close() : open(true, e.payload)));

// ------------------------------------------------------------------ live activities

const queue = [];
let actTimer = null;
let shownNotes = new Set();

function nextActivity() {
  if (mode !== "rest" || !queue.length) return;
  const a = queue.shift();
  $("actBadge").className = "badge " + (a.icon || "");
  $("actBadge").innerHTML = ICON[a.icon] || ICON.info;
  $("actTitle").textContent = a.title;
  $("actBody").textContent = a.body || "";
  const btns = $("actButtons");
  btns.replaceChildren();
  for (const act of a.actions || []) {
    const b = document.createElement("button");
    b.className = "chip" + (btns.children.length ? "" : " primary");
    b.textContent = act.label;
    b.addEventListener("click", () => {
      invoke("note_action", { kind: act.kind, target: act.target }).catch(() => {});
      clearTimeout(actTimer);
      setMode("rest");
      setTimeout(nextActivity, 520);
    });
    btns.append(b);
  }
  setMode("activity");
  const stay = (a.actions || []).length ? 7000 : 4200;
  actTimer = setTimeout(function done() {
    if (hovering && mode === "activity") { actTimer = setTimeout(done, 1500); return; }
    if (mode === "activity") { setMode("rest"); setTimeout(nextActivity, 520); }
  }, stay);
}

function iconForNote(n) {
  const t = (n.title || "").toLowerCase();
  if (t.includes("battery") || / is at \d+%/.test(n.title || "")) return "battery";
  if (t.includes("focus")) return "focus";
  if (t.includes("file") || t.includes("received") || t.includes("copied")) return "files";
  return "info";
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
$("cFind").addEventListener("click", (e) => { e.stopPropagation(); close(); setTimeout(() => invoke("island_find_pointer").catch(() => {}), 300); });
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
  const key = JSON.stringify([tools.clips, tools.shelf, tools.apps.map((a) => [a.name, a.apps.length])]);
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
  queue.push({ title: "Copied", body: c.kind === "text" ? c.text.slice(0, 120) : `${c.text}`, icon: "files" });
  setTimeout(close, 260);
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
      queue.push({ title: `Copying ${item.label}`, body: "It goes on the clipboard here. Paste it anywhere when it's ready.", icon: "files" });
      setTimeout(close, 500);
    });
    grid.append(t);
  }
}

function appMatches() {
  const q = $("appQuery").value.trim().toLowerCase();
  const out = [];
  for (const g of (tools && tools.apps) || []) {
    if (appWhere && g.name !== appWhere) continue;
    for (const a of g.apps) {
      const n = a.name.toLowerCase();
      let score = 0;
      if (!q) score = 1;
      else if (n.startsWith(q)) score = 3;
      else if (n.split(/[\s\-_.]+/).some((w) => w.startsWith(q))) score = 2;
      else if (n.includes(q)) score = 1;
      if (score) out.push({ on: g.name, app: a, score });
    }
  }
  if (q) out.sort((a, b) => b.score - a.score || a.app.name.localeCompare(b.app.name));
  return out.slice(0, 80);
}

function launch(m) {
  invoke("app_launch", { on: m.on, id: m.app.id }).catch(() => {});
  queue.push({ title: `Opening ${m.app.name}`, body: tools && m.on === tools.me ? "On this computer" : `On ${m.on}. The pointer is there now.`, icon: "info" });
  $("appQuery").value = "";
  close();
}

function renderApps() {
  const where = $("appWhere");
  where.replaceChildren();
  const groups = (tools && tools.apps) || [];
  if (groups.length > 1) {
    for (const [label, val] of [["Every computer", ""], ...groups.map((g) => [g.name === tools.me ? "This computer" : g.name, g.name])]) {
      const b = document.createElement("button");
      b.textContent = label;
      b.classList.toggle("on", appWhere === val);
      b.addEventListener("click", (e) => { e.stopPropagation(); appWhere = val; sel = 0; renderApps(); $("appQuery").focus(); });
      where.append(b);
    }
  }
  where.hidden = groups.length <= 1;
  const list = $("appList");
  list.replaceChildren();
  const items = appMatches();
  if (!items.length) {
    list.append(empty(groups.length ? "No app by that name." : "Apps from your computers show up here once OpenHop is on."));
    return;
  }
  sel = Math.min(sel, items.length - 1);
  items.forEach((m, i) => {
    const row = document.createElement("button");
    row.className = "row" + (i === sel ? " sel" : "");
    const lead = document.createElement("span");
    lead.className = "lead app";
    lead.style.background = colorFor(m.app.name);
    lead.textContent = m.app.name.trim().charAt(0).toUpperCase();
    const body = document.createElement("span");
    body.className = "body";
    const main = document.createElement("div");
    main.className = "main";
    main.textContent = m.app.name;
    body.append(main);
    const w = document.createElement("span");
    w.className = "where";
    w.textContent = tools && m.on === tools.me ? "This computer" : m.on;
    row.append(lead, body, w);
    row.addEventListener("click", (e) => { e.stopPropagation(); launch(m); });
    list.append(row);
  });
}
$("appQuery").addEventListener("input", () => { sel = 0; renderApps(); setMode("open"); });
$("appQuery").addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); moveSel($("appList"), e.key === "ArrowDown" ? 1 : -1); }
  else if (e.key === "Enter") { const m = appMatches()[sel]; if (m) launch(m); }
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
        queue.push({ title: `Sending ${itemsLabel(paths)}`, body: to === "*" ? "To all your computers" : `To ${to}`, icon: "files" });
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

async function poll() {
  try {
    snap = await invoke("island_state");
    for (const n of snap.notes || []) {
      if (shownNotes.has(n.id)) continue;
      shownNotes.add(n.id);
      queue.push({ title: n.title, body: n.body, icon: iconForNote(n), actions: n.actions });
    }
    for (const a of snap.activities || []) queue.push(a);
    render();
    if (mode === "rest") nextActivity();
  } catch (_) {}
  if (mode === "open" && tab !== "home" && tab !== "drop") refreshTools();
  setTimeout(poll, mode === "open" ? 450 : 900);
}
setTab("home");
island.style.setProperty("--w", REST.w + "px");
island.style.setProperty("--h", REST.h + "px");
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
    transfers: [{ label: "holiday.mp4", done: 40, total: 100 }],
    notes: [], activities: [],
  };
  let first = true;
  return async (cmd, args) => {
    if (cmd === "island_state") {
      const s = structuredClone(state);
      if (first && location.hash !== "#quiet") { s.activities = [{ title: "ubuntu-box is at 14%", body: "Plug it in soon.", icon: "battery" }]; first = false; }
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
        apps: [
          { name: "desk", apps: ["Calculator", "Firefox", "GIMP", "Terminal", "Visual Studio Code"].map((n, i) => ({ id: "d" + i, name: n })) },
          { name: "macbook", apps: ["Final Cut Pro", "Keynote", "Safari", "Terminal", "Xcode"].map((n, i) => ({ id: "m" + i, name: n })) },
        ],
      };
    }
    return null;
  };
}
