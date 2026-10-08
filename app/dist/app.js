"use strict";

const SERVER = "@server";
const OS_LABEL = { Windows: "Windows", MacOs: "macOS", Linux: "Linux", Other: "Other" };
const $ = (id) => document.getElementById(id);

// In the desktop app this is Tauri's IPC; in a plain browser a demo mock (for UI work).
const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : mockInvoke();

let snap = null;      // last snapshot from the backend
let form = null;      // editable copy of the config
let dirty = false;
let dragging = null;

// ------------------------------------------------------------------ helpers

function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v);
  }
  for (const k of kids) if (k != null) n.append(k);
  return n;
}

function setDirty(v) {
  dirty = v;
  $("save").disabled = !v;
  $("saveNote").textContent = v && snap?.status?.running ? "Saving briefly restarts sharing." : "";
}

// ------------------------------------------------------------------ settings form

function fillForm(cfg) {
  form = structuredClone(cfg);
  $("name").value = form.name;
  $("passphrase").value = form.passphrase;
  $("serverAddr").value = form.server_addr || "";
  $("swap").checked = form.swap_cmd_ctrl;
  $("clip").checked = form.clipboard_sync;
  $("notify").checked = form.notifications;
  $("wol").checked = form.wake_on_lan;
  $("port").value = form.port;
  $("speed").value = String(form.transfer_limit_mbps || 0);
  renderRole();
  fillPrefs(cfg);
}

// ------------------------------------------------------------------ instant settings

const PREF_ACCENTS = ["blue", "purple", "pink", "orange", "green", "graphite"];

function fillPrefs(cfg) {
  for (const n of document.querySelectorAll("[data-pref]")) {
    const v = cfg[n.dataset.pref];
    if (n.type === "checkbox") n.checked = !!v;
    else n.value = String(v ?? "");
  }
  renderLook(cfg.ui_appearance, cfg.ui_accent);
}

function renderLook(appearance, accent) {
  for (const b of document.querySelectorAll("#appearance button")) b.setAttribute("aria-checked", String(b.dataset.appearance === appearance));
  const box = $("accents");
  if (!box.children.length) {
    for (const a of PREF_ACCENTS) {
      const col = window.OpenHopTheme.ACCENTS[a][0];
      box.append(el("button", { type: "button", role: "radio", "aria-label": a, title: a[0].toUpperCase() + a.slice(1), style: `background:${col}`, "data-accent": a,
        onclick: () => setPref({ ui_accent: a }) }));
    }
  }
  for (const b of box.children) b.setAttribute("aria-checked", String(b.dataset.accent === accent));
  window.OpenHopTheme.apply(appearance, accent);
}

async function setPref(patch) {
  Object.assign(form, patch);
  if (snap) Object.assign(snap.config, patch);
  renderLook(form.ui_appearance, form.ui_accent);
  try { await invoke("update_prefs", { prefs: patch }); } catch (e) { showBanner(String(e)); }
}

for (const n of document.querySelectorAll("[data-pref]")) {
  n.addEventListener("change", () => {
    let v = n.type === "checkbox" ? n.checked : n.value;
    if (n.type === "range") v = parseInt(n.value, 10);
    setPref({ [n.dataset.pref]: v });
    if (n.dataset.pref === "sound" || n.dataset.pref === "sound_volume") invoke("play_sound", { kind: form.sound, volume: form.sound_volume });
  });
}
for (const b of document.querySelectorAll("#appearance button")) {
  b.addEventListener("click", () => setPref({ ui_appearance: b.dataset.appearance }));
}
$("soundTest").addEventListener("click", (e) => { e.preventDefault(); invoke("play_sound", { kind: form.sound === "off" ? "swoosh" : form.sound, volume: form.sound_volume }); });
$("openDock").addEventListener("click", () => invoke("dock_toggle"));

function renderRole() {
  document.body.classList.toggle("role-server", form.role === "server");
  document.body.classList.toggle("role-client", form.role === "client");
  for (const b of document.querySelectorAll(".seg button")) {
    b.setAttribute("aria-checked", String(b.dataset.role === form.role));
  }
  $("roleHint").textContent = form.role === "server"
    ? "This computer has the keyboard and mouse you want to use everywhere."
    : "Another computer's keyboard and mouse will control this one.";
}

function readForm() {
  form.name = $("name").value.trim() || form.name;
  form.passphrase = $("passphrase").value;
  form.server_addr = $("serverAddr").value.trim() || null;
  form.swap_cmd_ctrl = $("swap").checked;
  form.clipboard_sync = $("clip").checked;
  form.notifications = $("notify").checked;
  form.wake_on_lan = $("wol").checked;
  form.port = parseInt($("port").value, 10) || 24850;
  form.transfer_limit_mbps = parseInt($("speed").value, 10) || 0;
}

for (const id of ["name", "passphrase", "serverAddr", "swap", "clip", "notify", "wol", "port", "speed"]) {
  $(id).addEventListener("input", () => { readForm(); setDirty(true); });
}
for (const b of document.querySelectorAll(".seg button")) {
  b.addEventListener("click", () => { form.role = b.dataset.role; renderRole(); setDirty(true); render(); });
}
$("reveal").addEventListener("click", () => {
  const p = $("passphrase");
  const show = p.type === "password";
  p.type = show ? "text" : "password";
  $("reveal").textContent = show ? "Hide" : "Show";
});

$("save").addEventListener("click", async () => {
  readForm();
  try {
    await invoke("save_config", { config: form, restart: true });
    setDirty(false);
    await refresh();
  } catch (e) {
    showBanner(String(e));
  }
});

$("power").addEventListener("change", async () => {
  try {
    if (snap?.status?.running) {
      await invoke("stop");
    } else {
      if (dirty) {
        readForm();
        await invoke("save_config", { config: form, restart: false });
        setDirty(false);
      }
      await invoke("start");
    }
  } catch (e) {
    showBanner(String(e));
  }
  await refresh();
});

// ------------------------------------------------------------------ banner

function showBanner(text, action) {
  const b = $("banner");
  b.replaceChildren(text);
  if (action) b.append(el("button", { class: "link", onclick: action.run }, action.label));
  b.hidden = false;
}

function renderBanner() {
  const s = snap;
  if (s.permission) {
    return showBanner(s.permission, { label: "Open settings", run: () => invoke("request_permission") });
  }
  const err = s.error || s.status?.error;
  if (err) return showBanner(err);
  if (form.role === "server" && s.wayland) {
    return showBanner("This Linux desktop runs Wayland. Sharing its keyboard and mouse needs an Xorg session; it can still be controlled as a client.");
  }
  $("banner").hidden = true;
}

// ------------------------------------------------------------------ layout grid

function placeIn(layout, name, x, y) {
  // Mirrors Layout::place in Rust: dropping onto an occupied cell swaps.
  if (name === SERVER || (x === 0 && y === 0)) return layout;
  const screens = layout.screens.filter((s) => s.name !== name);
  const old = layout.screens.find((s) => s.name === name);
  const i = screens.findIndex((s) => s.x === x && s.y === y);
  if (i >= 0) {
    const d = screens.splice(i, 1)[0];
    if (old) screens.push({ name: d.name, x: old.x, y: old.y });
  }
  screens.push({ name, x, y });
  return { screens };
}

function renderGrid() {
  const st = snap.status;
  const layout = (st && st.role === "server") ? st.layout : form.layout;
  const peers = new Map((st?.peers || []).map((p) => [p.name, p]));
  const cells = [{ name: SERVER, x: 0, y: 0 }, ...layout.screens];
  const xs = cells.map((c) => c.x), ys = cells.map((c) => c.y);
  const x0 = Math.min(...xs) - 1, x1 = Math.max(...xs) + 1;
  const y0 = Math.min(...ys) - 1, y1 = Math.max(...ys) + 1;

  const grid = $("grid");
  grid.style.gridTemplateColumns = `repeat(${x1 - x0 + 1}, minmax(56px, var(--cellw)))`;
  grid.replaceChildren();
  for (let y = y0; y <= y1; y++) {
    for (let x = x0; x <= x1; x++) {
      const cell = el("div", { class: "cell" });
      cell.dataset.x = x; cell.dataset.y = y;
      const who = cells.find((c) => c.x === x && c.y === y);
      if (who) cell.append(tile(who.name, peers, st));
      grid.append(cell);
    }
  }
}

function tile(name, peers, st) {
  const self = name === SERVER;
  const p = peers.get(name);
  const active = st?.running && ((self && !st.active) || st.active === name);
  const os = self ? snap.os : p?.os;
  const cls = ["tile", self ? "self" : "", os ? `os-${os}` : "", !self && !p ? "offline" : "", active && peers.size ? "active" : ""].join(" ");
  const screen = self ? st?.screen : p?.screen;
  const t = el("div", { class: cls },
    el("span", { class: "os" }, OS_LABEL[os] || ""),
    active && peers.size ? el("span", { class: "here", title: "Cursor is here" }) : null,
    el("b", {}, self ? `${form.name} (this)` : name),
    el("small", {}, screen && screen.w ? `${screen.w}×${screen.h}` : (self ? "" : (st?.wakeable || []).includes(name) ? "Asleep" : "Offline")),
  );
  if (!self) t.addEventListener("pointerdown", (e) => startDrag(e, t, name));
  return t;
}

function startDrag(e, t, name) {
  e.preventDefault();
  const ghost = t.cloneNode(true);
  ghost.classList.add("dragging");
  document.body.append(ghost);
  t.style.opacity = "0.25";
  dragging = { name, ghost, over: null };
  $("grid").classList.add("drag-on");
  const move = (ev) => {
    ghost.style.left = `${ev.clientX - 70}px`;
    ghost.style.top = `${ev.clientY - 44}px`;
    const under = document.elementFromPoint(ev.clientX, ev.clientY)?.closest(".cell");
    if (dragging.over && dragging.over !== under) dragging.over.classList.remove("drop");
    if (under) under.classList.add("drop");
    dragging.over = under;
  };
  const up = async () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    ghost.remove();
    $("grid").classList.remove("drag-on");
    const target = dragging.over;
    dragging = null;
    if (target) {
      target.classList.remove("drop");
      const x = +target.dataset.x, y = +target.dataset.y;
      const st = snap.status;
      const current = (st && st.role === "server") ? st.layout : form.layout;
      const next = placeIn(current, name, x, y);
      form.layout = next;
      if (st) st.layout = next;
      try { await invoke("set_layout", { layout: next }); } catch (err) { showBanner(String(err)); }
    }
    render();
  };
  move(e);
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}

// ------------------------------------------------------------------ status

function renderStatus() {
  const st = snap.status;
  const running = !!st?.running;
  $("power").checked = running;
  const dot = $("statusDot");
  dot.className = "dot " + (running ? (st.peers.length ? "on" : "busy") : "");
  $("statusText").textContent = running ? st.message : "Off";

  const isClient = form.role === "client";
  $("grid").hidden = isClient;
  $("clientView").hidden = !isClient;
  $("stageTitle").textContent = isClient ? "Status" : "Arrangement";
  $("stageHint").textContent = isClient
    ? "Screens are arranged on the computer that shares its keyboard and mouse."
    : "Drag screens to match where they sit on your desk, then move the pointer off an edge to hop across.";
  if (isClient) {
    const server = st?.peers?.[0];
    const here = running && st.active;
    $("clientView").querySelector(".display").classList.toggle("active", !!here);
    $("clientLine").textContent = !running ? "Not running"
      : server ? (here ? `Controlled by ${server.name} right now` : `Connected to ${server.name}`)
      : st.message;
  }
}

function renderNearby() {
  const list = $("nearby");
  const st = snap.status;
  const items = st?.discovered || [];
  const connected = new Set((st?.peers || []).map((p) => p.name));
  const paired = new Set((st?.paired || []).map((p) => p.device));
  list.replaceChildren();
  if (!st?.running) {
    list.append(el("div", { class: "item empty" }, "Turn on OpenHop to look for other computers."));
    return;
  }
  if (form.role === "client" && st.needs_pairing) {
    list.append(el("div", { class: "hint-row" }, "Pick the computer whose keyboard and mouse you want to use, then enter the code it shows."));
  }
  if (!items.length) {
    list.append(el("div", { class: "item empty" }, "Searching… Open OpenHop on your other computers (same network, or joined by a cable)."));
    return;
  }
  const initials = { MacOs: "MAC", Windows: "WIN", Linux: "LNX", Other: "PC" };
  for (const d of items) {
    const live = connected.has(d.name);
    const isPaired = paired.has(d.device);
    let right;
    if (live) {
      right = el("span", { class: "tag live" }, "Connected");
    } else if (form.role === "client" && d.role === "server") {
      right = isPaired
        ? el("span", { class: "tag" }, "Paired")
        : el("button", { class: "pill", onclick: () => openPair(d) }, "Pair");
    } else {
      right = el("span", { class: "tag" }, isPaired ? "Paired" : d.role === "server" ? "Sharing" : "Waiting");
    }
    list.append(el("div", { class: "item" },
      el("span", { class: `icon os-${d.os}` }, initials[d.os] || "PC"),
      el("span", { class: "text" },
        el("b", {}, d.name, d.link === "direct" ? el("span", { class: "badge-direct" }, "Cable") : null),
        el("span", {}, `${OS_LABEL[d.os]} · ${d.link === "direct" ? "direct connection" : d.addr.replace(/:\d+$/, "").replace(/^\[|\]$/g, "")}`)),
      right,
    ));
  }
}

// ------------------------------------------------------------------ nearby radar

let radarKey = "";

function renderRadar() {
  const st = snap.status;
  const radar = $("radar");
  const running = !!st?.running;
  const items = (st?.discovered || []).slice().sort((a, b) => (a.device || a.name).localeCompare(b.device || b.name));
  const connected = new Set((st?.peers || []).map((p) => p.name));
  const paired = new Set((st?.paired || []).map((p) => p.device));
  const isClient = form.role === "client";
  const state = (d) => connected.has(d.name) ? "live" : paired.has(d.device) ? "paired" : (isClient && d.role === "server") ? "pair" : "seen";
  const key = JSON.stringify([running, isClient, form.name, snap.os, items.map((d) => [d.device, d.name, d.os, d.role, state(d)])]);
  if (key === radarKey) return;
  radarKey = key;
  radar.classList.toggle("idle", !running);
  radar.querySelectorAll(".peer, .me, .guide, .caption").forEach((n) => n.remove());
  const initials = { MacOs: "MAC", Windows: "WIN", Linux: "LNX", Other: "PC" };
  const w = radar.clientWidth || 600, h = 300, cx = w / 2, cy = 140;
  for (const r of [92, 128]) {
    radar.append(el("span", { class: "guide", style: `width:${r * 2}px;height:${r * 2}px;top:${cy}px` }));
  }
  radar.append(el("div", { class: "me", style: `top:${cy}px` },
    el("div", { class: "avatar" }, initials[snap.os] || "PC"),
    el("span", {}, form.name || "This computer")));
  const n = items.length;
  items.forEach((d, i) => {
    const orbit = n > 6 && i % 2 ? 128 : (n > 6 ? 92 : 112);
    const angle = (-90 + 35 + (360 / Math.max(n, 1)) * i) * Math.PI / 180;
    const x = cx + Math.cos(angle) * orbit * 1.55, y = cy + Math.sin(angle) * orbit * 0.95;
    const s = state(d);
    const label = s === "live" ? "Connected" : s === "paired" ? "Paired" : s === "pair" ? "Tap to pair" : d.role === "server" ? "Sharing" : "Waiting";
    const node = el(s === "pair" ? "button" : "div", {
      class: `peer os-${d.os} ${s}${s === "pair" ? " can-pair" : ""}`,
      style: `left:${Math.round(x)}px;top:${Math.round(y)}px`,
      title: `${d.name} · ${OS_LABEL[d.os] || ""}`,
    },
      el("div", { class: "avatar" }, initials[d.os] || "PC"),
      el("span", { class: "nm" }, d.name),
      el("span", { class: "st" + (s === "pair" ? " cta" : "") }, label));
    if (s === "pair") node.addEventListener("click", () => openPair(d));
    radar.append(node);
  });
  const caption = !running ? "Turn on OpenHop to look for computers nearby."
    : n === 0 ? "Looking for computers nearby… Open OpenHop on your other computers."
    : isClient && st.needs_pairing ? "Tap the computer whose keyboard and mouse you want to use."
    : "";
  if (caption) radar.append(el("div", { class: "caption" }, caption));
}
window.addEventListener("resize", () => { radarKey = ""; if (snap) renderRadar(); });

// ------------------------------------------------------------------ pairing

let pairing = null; // the discovered server being paired with

function openPair(d) {
  pairing = { device: d ? d.device : null, name: d ? d.name : null, byAddr: !d, sent: false, peers0: (snap.status?.peers || []).length };
  $("pairTitle").textContent = d ? `Pair with ${d.name}` : "Connect by Address";
  $("pairText").textContent = d
    ? `Enter the 6-digit code shown in OpenHop on ${d.name}.`
    : "On the other computer, OpenHop shows its address and a 6-digit code under Pair a Computer.";
  for (const id of ["addrInput", "addrLabel", "codeLabel"]) $(id).hidden = !!d;
  $("addrInput").value = snap.config?.server_addr || "";
  $("pairInput").value = "";
  $("pairError").textContent = "";
  $("pairGo").disabled = false;
  $("pairGo").textContent = d ? "Pair" : "Connect";
  $("pairSheet").hidden = false;
  setTimeout(() => (d ? $("pairInput") : $("addrInput")).focus(), 50);
}

function closePair() {
  pairing = null;
  $("pairSheet").hidden = true;
}

async function submitPair() {
  const code = $("pairInput").value.replace(/\D/g, "");
  const addr = $("addrInput").value.trim();
  if (pairing.byAddr && !addr) {
    $("pairError").textContent = "Type the address shown on the other computer.";
    return;
  }
  if (code.length !== 6) {
    $("pairError").textContent = "The code has 6 digits.";
    return;
  }
  $("pairError").textContent = "";
  $("pairGo").disabled = true;
  $("pairGo").textContent = pairing.byAddr ? "Connecting…" : "Pairing…";
  pairing.sent = true;
  pairing.at = Date.now();
  try {
    if (pairing.byAddr) await invoke("pair_addr", { addr, code });
    else await invoke("pair", { device: pairing.device, code });
  } catch (e) {
    $("pairError").textContent = String(e);
    $("pairGo").disabled = false;
    $("pairGo").textContent = pairing.byAddr ? "Connect" : "Pair";
  }
}

$("pairCancel").addEventListener("click", closePair);
$("pairGo").addEventListener("click", submitPair);
$("byAddr").addEventListener("click", () => openPair(null));
for (const id of ["pairInput", "addrInput"]) {
  $(id).addEventListener("keydown", (e) => { if (e.key === "Enter") submitPair(); if (e.key === "Escape") closePair(); });
}
$("pairInput").addEventListener("input", () => {
  const d = $("pairInput").value.replace(/\D/g, "").slice(0, 6);
  $("pairInput").value = d.length > 3 ? `${d.slice(0, 3)} ${d.slice(3)}` : d;
});

function renderPairing() {
  const st = snap.status;
  const code = st?.pairing_code;
  $("pairCode").textContent = code ? `${code.slice(0, 3)} ${code.slice(3)}` : "––– –––";
  const pairedList = st?.paired || [];
  $("pairedBlock").hidden = !pairedList.length;
  const box = $("pairedList");
  box.replaceChildren();
  for (const p of pairedList) {
    box.append(el("div", { class: "row" },
      el("span", { class: "label" }, p.name),
      el("button", { class: "link", onclick: async () => { await invoke("forget", { device: p.device }); refresh(); } }, "Forget"),
    ));
  }
  $("myAddr").textContent = (st?.addresses || []).join(", ") || "–";
  // Follow a pairing attempt in progress.
  if (pairing?.sent && st) {
    const done = pairing.byAddr
      ? (st.peers || []).length > 0 && !st.pairing_with && Date.now() - pairing.at > 800
      : pairedList.some((p) => p.device === pairing.device);
    if (done) {
      closePair();
    } else if (st.pair_error && !st.pairing_with && Date.now() - pairing.at > 800) {
      $("pairError").textContent = st.pair_error;
      $("pairGo").disabled = false;
      $("pairGo").textContent = pairing.byAddr ? "Connect" : "Pair";
      pairing.sent = false;
    }
  }
}

// ------------------------------------------------------------------ updates

$("updateBtn").addEventListener("click", async () => { await invoke("update_check"); refresh(); });
$("installBtn").addEventListener("click", async () => { await invoke("update_install"); refresh(); });

function renderUpdate() {
  const u = snap.update || {};
  $("version").textContent = snap.version || "";
  const row = $("updateRow");
  const note = $("updateNote");
  row.hidden = true;
  note.textContent = "";
  $("updateBtn").disabled = u.phase === "checking" || u.phase === "downloading" || u.phase === "installing";
  switch (u.phase) {
    case "checking": note.textContent = "Checking…"; break;
    case "current": note.textContent = "You're up to date."; break;
    case "available":
      row.hidden = false;
      $("updateText").textContent = `Version ${u.latest} is available`;
      $("installBtn").disabled = false;
      note.textContent = "Update every computer to the same version.";
      break;
    case "downloading":
      row.hidden = false;
      $("updateText").textContent = `Downloading ${u.latest || ""}… ${Math.round((u.progress || 0) * 100)}%`;
      $("installBtn").disabled = true;
      break;
    case "installing":
      row.hidden = false;
      $("updateText").textContent = "Installing… OpenHop will reopen.";
      $("installBtn").disabled = true;
      break;
    case "error": note.textContent = u.message || "Update failed."; break;
  }
}

function fmtBytes(b) {
  if (b >= 1 << 30) return (b / (1 << 30)).toFixed(1) + " GB";
  if (b >= 1 << 20) return (b / (1 << 20)).toFixed(1) + " MB";
  if (b >= 1 << 10) return Math.round(b / 1024) + " KB";
  return b + " B";
}

function renderTransfers() {
  const items = (snap.status?.transfers || []).slice(-5).reverse();
  $("transfersBlock").hidden = !items.length;
  const box = $("transfers");
  box.replaceChildren();
  for (const t of items) {
    const pct = t.total ? Math.min(100, Math.round((t.done / t.total) * 100)) : 100;
    const state = t.error ? "Failed" : t.finished ? "Done" : `${pct}%`;
    const bar = el("div", { class: "bar" }, el("span", { style: `width:${t.error ? 100 : pct}%` }));
    if (t.error) bar.classList.add("err");
    else if (t.finished) bar.classList.add("ok");
    box.append(el("div", { class: "item transfer" },
      el("span", { class: "icon file" }, "⇣"),
      el("span", { class: "text" },
        el("b", {}, t.label),
        el("span", {}, t.error ? t.error : `From ${t.peer} · ${fmtBytes(t.total)}`),
        bar),
      el("span", { class: "tag" + (t.finished && !t.error ? " live" : "") }, state),
    ));
  }
}

function render() {
  if (!snap) return;
  renderTransfers();
  renderPairing();
  renderUpdate();
  renderRadar();
  renderBanner();
  renderStatus();
  if (form.role === "server" && !dragging) renderGrid();
  renderNearby();
}

async function refresh() {
  try {
    snap = await invoke("snapshot");
  } catch (e) {
    showBanner(String(e));
    return;
  }
  if (!form) fillForm(snap.config);
  else {
    // Instant settings are saved as they change; keep them in step.
    for (const k of ["theme_sync", "dnd_sync", "sound", "sound_volume", "battery_saver", "stream_quality", "window_drag", "ui_appearance", "ui_accent", "open_at_login"]) form[k] = snap.config[k];
  }
  if (form && !dirty) {
    // Pick up layout changes the server made (newly connected computers).
    form.layout = snap.config.layout;
  }
  render();
}

refresh();
setInterval(refresh, 800);

// ------------------------------------------------------------------ demo mode

function mockInvoke() {
  const cfg = {
    name: "desk-pc", role: "server", passphrase: "correct horse", port: 24850, server_addr: null, server_name: null,
    layout: { screens: [{ name: "macbook", x: 1, y: 0 }, { name: "ubuntu-box", x: -1, y: 0 }, { name: "imac", x: 0, y: -1 }] },
    swap_cmd_ctrl: true, clipboard_sync: true, screen: null, linux_backend: "auto",
    notifications: true, wake_on_lan: true, macs: {}, last_ips: {}, download_dir: null,
    transfer_limit_mbps: 50, device_id: "a1", trusted: {}, server_device: null,
    theme_sync: true, dnd_sync: true, sound: "swoosh", sound_volume: 60, battery_saver: true, stream_quality: "balanced",
    window_drag: true, ui_appearance: "system", ui_accent: "blue", open_at_login: true,
  };
  let running = true;
  if (location.search.includes("client")) {
    Object.assign(cfg, { name: "macbook", role: "client", layout: { screens: [] }, passphrase: "", transfer_limit_mbps: 0, notifications: true, wake_on_lan: true });
    return async (cmd, args) => {
      if (cmd === "snapshot") return { config: structuredClone(cfg), os: "MacOs", error: null, permission: null, wayland: false,
        status: running ? { running: true, role: "client", name: "macbook", os: "MacOs", message: "Connected to desk-pc", error: null,
          active: "macbook", peers: [{ name: "desk-pc", os: "Windows", addr: "192.168.1.20", screen: { x: 0, y: 0, w: 0, h: 0 } }],
          discovered: [{ name: "desk-pc", os: "Windows", role: "server", addr: "192.168.1.20:24850", device: "d1", link: "network" },
                       { name: "studio-pc", os: "Linux", role: "server", addr: "[fe80::1%3]:24850", device: "s9", link: "direct" },
                       { name: "ubuntu-box", os: "Linux", role: "client", addr: "192.168.1.40:24850", device: "u2", link: "network" }],
          paired: [{ device: "d1", name: "desk-pc" }], needs_pairing: false, pairing_with: null, pair_error: null, transfers: [],
          layout: { screens: [] }, screen: { x: 0, y: 0, w: 1512, h: 982 } } : null,
        version: "0.3.0", update: { phase: "current", current: "0.3.0" } };
      if (cmd === "stop") running = false;
      if (cmd === "start") running = true;
      if (cmd === "save_config") Object.assign(cfg, args.config);
      return null;
    };
  }
  const status = () => running ? {
    running: true, role: cfg.role, name: cfg.name, os: "Windows",
    message: "Sharing with 2 computer(s)", error: null, active: "macbook",
    peers: [
      { name: "macbook", os: "MacOs", addr: "192.168.1.31", screen: { x: 0, y: 0, w: 1512, h: 982 } },
      { name: "ubuntu-box", os: "Linux", addr: "192.168.1.40", screen: { x: 0, y: 0, w: 2560, h: 1440 } },
    ],
    discovered: [
      { name: "macbook", os: "MacOs", role: "client", addr: "192.168.1.31:24850", device: "b2", link: "network" },
      { name: "ubuntu-box", os: "Linux", role: "client", addr: "169.254.20.2:24850", link: "direct", device: "c3" },
    ],
    layout: cfg.layout, screen: { x: 0, y: 0, w: 2560, h: 1440 },
    wakeable: ["imac"],
    device: "a1", pairing_code: "485632", needs_pairing: false, pairing_with: null, pair_error: null, addresses: ["192.168.1.20"], port: 24850,
    paired: [{ device: "b2", name: "macbook" }, { device: "c3", name: "ubuntu-box" }],
    transfers: [
      { offer: 1, label: "holiday video.mp4", peer: "macbook", incoming: true, done: 912 * 1048576, total: 1450 * 1048576, finished: false, error: null },
      { offer: 2, label: "design.psd", peer: "ubuntu-box", incoming: true, done: 48 * 1048576, total: 48 * 1048576, finished: true, error: null },
    ],
  } : null;
  return async (cmd, args) => {
    switch (cmd) {
      case "snapshot": return { config: structuredClone(cfg), status: status(), os: "Windows", error: null, permission: null, wayland: false,
        version: "0.3.0", update: { phase: "available", current: "0.3.0", latest: "0.3.1" } };
      case "save_config": Object.assign(cfg, args.config); return null;
      case "set_layout": cfg.layout = args.layout; return null;
      case "update_prefs": Object.assign(cfg, args.prefs); return null;
      case "start": running = true; return null;
      case "stop": running = false; return null;
      default: return null;
    }
  };
}
