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
  $("port").value = form.port;
  renderRole();
}

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
  form.port = parseInt($("port").value, 10) || 24850;
}

for (const id of ["name", "passphrase", "serverAddr", "swap", "clip", "port"]) {
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
    el("small", {}, screen && screen.w ? `${screen.w}×${screen.h}` : (self ? "" : "Offline")),
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
  list.replaceChildren();
  if (!st?.running) {
    list.append(el("div", { class: "item empty" }, "Turn on OpenHop to look for other computers."));
    return;
  }
  if (!items.length) {
    list.append(el("div", { class: "item empty" }, "Searching… Install OpenHop on your other computers and use the same passphrase."));
    return;
  }
  const initials = { MacOs: "MAC", Windows: "WIN", Linux: "LNX", Other: "PC" };
  for (const d of items) {
    const live = connected.has(d.name);
    list.append(el("div", { class: "item" },
      el("span", { class: `icon os-${d.os}` }, initials[d.os] || "PC"),
      el("span", { class: "text" }, el("b", {}, d.name), el("span", {}, `${OS_LABEL[d.os]} · ${d.addr.split(":")[0]}`)),
      el("span", { class: "tag" + (live ? " live" : "") }, live ? "Connected" : d.role === "server" ? "Sharing" : "Waiting"),
    ));
  }
}

function render() {
  if (!snap) return;
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
  else if (!dirty) {
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
    layout: { screens: [{ name: "macbook", x: 1, y: 0 }, { name: "ubuntu-box", x: -1, y: 0 }] },
    swap_cmd_ctrl: true, clipboard_sync: true, screen: null, linux_backend: "auto",
  };
  let running = true;
  if (location.search.includes("client")) {
    Object.assign(cfg, { name: "macbook", role: "client", layout: { screens: [] } });
    return async (cmd, args) => {
      if (cmd === "snapshot") return { config: structuredClone(cfg), os: "MacOs", error: null, permission: null, wayland: false,
        status: running ? { running: true, role: "client", name: "macbook", os: "MacOs", message: "Connected to desk-pc", error: null,
          active: "macbook", peers: [{ name: "desk-pc", os: "Windows", addr: "192.168.1.20", screen: { x: 0, y: 0, w: 0, h: 0 } }],
          discovered: [{ name: "desk-pc", os: "Windows", role: "server", addr: "192.168.1.20:24850" },
                       { name: "ubuntu-box", os: "Linux", role: "client", addr: "192.168.1.40:24850" }],
          layout: { screens: [] }, screen: { x: 0, y: 0, w: 1512, h: 982 } } : null };
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
      { name: "macbook", os: "MacOs", role: "client", addr: "192.168.1.31:24850" },
      { name: "ubuntu-box", os: "Linux", role: "client", addr: "192.168.1.40:24850" },
    ],
    layout: cfg.layout, screen: { x: 0, y: 0, w: 2560, h: 1440 },
  } : null;
  return async (cmd, args) => {
    switch (cmd) {
      case "snapshot": return { config: structuredClone(cfg), status: status(), os: "Windows", error: null, permission: null, wayland: false };
      case "save_config": Object.assign(cfg, args.config); return null;
      case "set_layout": cfg.layout = args.layout; return null;
      case "start": running = true; return null;
      case "stop": running = false; return null;
      default: return null;
    }
  };
}
