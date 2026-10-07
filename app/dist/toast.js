"use strict";

const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : demo();
const stack = document.getElementById("stack");
const SHOW_MS = 9000;

function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v);
  }
  for (const c of kids) if (c != null) n.append(c);
  return n;
}

function layout() {
  // Let the window hug its content (and hide when empty).
  // (Not requestAnimationFrame: it never fires while the window is hidden.)
  setTimeout(() => {
    const h = stack.children.length ? stack.getBoundingClientRect().height + 16 : 0;
    invoke("toast_layout", { height: Math.ceil(h) });
  }, 0);
}

function dismiss(card) {
  if (card.classList.contains("out")) return;
  card.classList.add("out");
  setTimeout(() => { card.remove(); layout(); }, 220);
}

function show(note) {
  const card = el("div", { class: "toast" });
  const close = el("button", { class: "close", title: "Dismiss", onclick: () => dismiss(card) }, "✕");
  card.append(
    close,
    el("div", { class: "head" }, el("img", { src: "icon.png", alt: "" }), "OpenHop", el("span", { class: "when" }, "now")),
    el("div", { class: "title" }, note.title),
    note.body ? el("div", { class: "body" }, note.body) : null,
  );
  if (note.actions.length) {
    const row = el("div", { class: "actions" });
    for (const a of note.actions) {
      row.append(el("button", {
        onclick: async () => {
          try { await invoke("note_action", { kind: a.kind, target: a.target }); } catch (e) { console.error(e); }
          dismiss(card);
        },
      }, a.label));
    }
    card.append(row);
  }
  stack.prepend(card);
  while (stack.children.length > 3) stack.lastChild.remove();
  let timer = setTimeout(() => dismiss(card), SHOW_MS);
  card.addEventListener("mouseenter", () => clearTimeout(timer));
  card.addEventListener("mouseleave", () => { timer = setTimeout(() => dismiss(card), 3000); });
  layout();
}

async function poll() {
  try {
    const notes = await invoke("toast_queue");
    for (const n of notes) show(n);
  } catch (e) {
    console.error(e);
  }
}
setInterval(poll, 300);

function demo() {
  const sample = [
    { id: 1, title: "Received design.psd", body: "From macbook. Saved to Downloads › OpenHop.", actions: [{ label: "Open", kind: "open_path", target: "" }, { label: "Show in folder", kind: "reveal", target: "" }] },
    { id: 2, title: "Link copied on desk-pc", body: "https://github.com/maykano-dev/open-hop", actions: [{ label: "Open", kind: "open_url", target: "" }] },
  ];
  return async (cmd) => (cmd === "toast_queue" ? sample.splice(0) : null);
}
