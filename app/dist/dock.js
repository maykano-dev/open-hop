"use strict";
// The window dock: every window open on every computer. Click one to open it
// live on this computer (or bring it forward if it's already here).

const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : demo();
const rows = document.getElementById("rows");
let me = "";
let lastKey = "";

function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k === "style") n.style.cssText = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v);
  }
  for (const c of kids) if (c != null) n.append(c);
  return n;
}

function color(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hues = [211, 262, 340, 28, 145, 190, 8, 48];
  const hue = hues[h % hues.length];
  return `hsl(${hue}, 75%, 55%); background-image: linear-gradient(145deg, hsl(${hue}, 80%, 62%), hsl(${hue + 18}, 72%, 48%))`;
}

async function open(computer, w) {
  try {
    await invoke("open_window", { origin: computer, window: String(w.id) });
  } catch (e) { console.warn(e); }
  invoke("dock_hide");
}

function render(snap) {
  const st = snap.status;
  me = st ? st.name : "";
  const groups = st ? st.windows : [];
  const key = JSON.stringify(groups);
  if (key === lastKey) return;
  lastKey = key;
  rows.replaceChildren();
  if (!st) { rows.append(el("p", { class: "empty" }, "Turn OpenHop on to see windows from your other computers.")); return; }
  if (!groups.length) { rows.append(el("p", { class: "empty" }, "No windows yet. Other computers' windows appear here once they're connected.")); return; }
  for (const g of groups) {
    const here = g.name === me;
    rows.append(el("div", { class: "computer" },
      el("div", { class: "who" }, here ? el("b", {}, "This Computer") : g.name),
      el("div", { class: "tiles" }, ...g.windows.map((w) =>
        el("button", { class: "tile", title: `${w.title}${w.app ? " — " + w.app : ""}`, onclick: () => open(g.name, w) },
          el("span", { class: "badge", style: `background-color:${color(w.app || w.title)}` }, (w.app || w.title || "?").trim().charAt(0).toUpperCase()),
          el("span", { class: "name" }, w.title))))));
  }
}

async function refresh() {
  try {
    const s = await invoke("snapshot");
    window.OpenHopTheme.apply(null, s.config.ui_accent, s.system_dark);
    render(s);
  } catch (e) { console.warn(e); }
}

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") invoke("dock_hide");
  const tiles = [...document.querySelectorAll(".tile")];
  const i = tiles.indexOf(document.activeElement);
  if (e.key === "ArrowRight" && tiles.length) { tiles[Math.min(tiles.length - 1, i + 1)].focus(); e.preventDefault(); }
  if (e.key === "ArrowLeft" && tiles.length) { tiles[Math.max(0, i - 1)].focus(); e.preventDefault(); }
});
if (window.__TAURI__) window.__TAURI__.event.listen("shown", () => { lastKey = ""; refresh(); });
refresh();
setInterval(refresh, 1000);

function demo() {
  const snap = { config: { ui_accent: "blue" }, status: { name: "desk-pc", windows: [
    { name: "desk-pc", windows: [{ id: 1, title: "Inbox — Mail", app: "Thunderbird" }, { id: 2, title: "main.rs — open-hop", app: "Code" }] },
    { name: "macbook", windows: [{ id: 3, title: "Design review.key", app: "Keynote" }, { id: 4, title: "Spotify", app: "Spotify" }, { id: 5, title: "Safari — OpenHop", app: "Safari" }] },
    { name: "ubuntu-box", windows: [{ id: 6, title: "Terminal", app: "gnome-terminal" }, { id: 7, title: "Files", app: "Nautilus" }] },
  ] } };
  return async (cmd) => (cmd === "snapshot" ? snap : null);
}
