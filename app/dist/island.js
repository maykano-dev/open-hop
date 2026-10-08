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
  info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.5"/></svg>',
};
const ACCENTS = { blue: "#0a84ff", purple: "#bf5af2", pink: "#ff375f", orange: "#ff9f0a", green: "#30d158", graphite: "#98989d" };

$("openApp").innerHTML = ICON.gear;
$("cFocus").querySelector(".disc").innerHTML = ICON.moon;
$("cFind").querySelector(".disc").innerHTML = ICON.find;
$("cLock").querySelector(".disc").innerHTML = ICON.lock;
$("cSleep").querySelector(".disc").innerHTML = ICON.power;

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

function open(byShortcut) {
  pinned = !!byShortcut;
  render();
  setMode("open");
}
function close() {
  pinned = false;
  setMode("rest");
  setTimeout(nextActivity, 520);
}

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
if (T) T.event.listen("toggle", () => (mode === "open" ? close() : open(true)));

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
  $("headTitle").textContent = s.running ? "OpenHop" : "OpenHop is off";
  $("headSub").textContent = s.running ? s.message : "Open OpenHop to turn it on.";
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
      const card = document.createElement("div");
      card.className = "pc" + (isHere ? " here" : "");
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
  setTimeout(poll, mode === "open" ? 450 : 900);
}
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
    return null;
  };
}
