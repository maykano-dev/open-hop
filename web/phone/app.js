"use strict";
// OpenHop on a phone. Two ways to a computer:
//  - direct (this page on HTTPS): the phone and computer meet through public
//    relays, with everything sealed with the secret from the QR code, then
//    talk over WebRTC, with a second layer of encryption inside;
//  - same Wi-Fi (the page served by the computer itself, http://…/<key>/).
// Both carry the same requests.

const $ = (id) => document.getElementById(id);
const enc = new TextEncoder(), dec = new TextDecoder();
const LAN = location.protocol === "http:" && /^\/[0-9a-f]{32}\/$/.test(location.pathname);
const IOS = /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
const ANDROID = /Android/.test(navigator.userAgent);
const DEVICE = IOS ? (/iPad/.test(navigator.userAgent) ? "iPad" : "iPhone") : ANDROID ? "Android phone" : "Phone";
const RELAYS = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net", "wss://nostr.mom", "wss://relay.snort.social"];
const STUN = [{ urls: ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"] }];
const KIND = 21071;
const CHUNK = 60 * 1024;

// ------------------------------------------------------------------ icons

const ICON = {
  photo: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="3" y="4.5" width="18" height="15" rx="3.5"/><circle cx="9" cy="10" r="1.8"/><path d="m4 18 5-4.6 3.6 3.2 2.9-2.6L21 18.5"/></svg>',
  file: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M6.5 2.8h7.2l4.8 4.8v13.6H6.5z"/><path d="M13.5 3v5h5"/></svg>',
  text: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M5 6.5h14M5 11.5h14M5 16.5h8.5"/></svg>',
  send: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M6 11l6-6 6 6"/></svg>',
  up: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 15v3.5A1.5 1.5 0 0 0 5.5 20h13a1.5 1.5 0 0 0 1.5-1.5V15"/></svg>',
  tray: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 13.5 5.6 5.8A2 2 0 0 1 7.5 4.5h9a2 2 0 0 1 1.9 1.3L21 13.5V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M3 13.5h5l1.5 2.5h5l1.5-2.5h5"/></svg>',
  clip: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="5" y="4.5" width="14" height="17" rx="2.5"/><path d="M9 4.5V3.3h6v1.2M9 10h6M9 14h6M9 18h3.5" stroke-linecap="round"/></svg>',
  remote: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="4" y="3" width="16" height="18" rx="4"/><path d="M12 3v7M4 10h16" /></svg>',
  laptop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="5" width="16" height="11" rx="1.6"/><path d="M2 19h20"/></svg>',
  all: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="2.5" y="8" width="11" height="8" rx="1.4"/><rect x="10.5" y="4" width="11" height="8" rx="1.4"/><path d="M5 20h6" stroke-linecap="round"/></svg>',
  search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m20 20-4.6-4.6"/></svg>',
  play: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.6v14.8a1 1 0 0 0 1.5.86l12.3-7.4a1 1 0 0 0 0-1.72L8.5 3.74A1 1 0 0 0 7 4.6z"/></svg>',
  pause: '<svg viewBox="0 0 24 24" fill="currentColor"><rect x="5.5" y="4" width="4.6" height="16" rx="1.4"/><rect x="13.9" y="4" width="4.6" height="16" rx="1.4"/></svg>',
  next: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M2.5 6.2v11.6a.8.8 0 0 0 1.25.66L12 12.66v5.14a.8.8 0 0 0 1.25.66l8.6-5.8a.8.8 0 0 0 0-1.32l-8.6-5.8A.8.8 0 0 0 12 6.2v5.14L3.75 5.54a.8.8 0 0 0-1.25.66z"/></svg>',
  prev: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M21.5 6.2v11.6a.8.8 0 0 1-1.25.66L12 12.66v5.14a.8.8 0 0 1-1.25.66l-8.6-5.8a.8.8 0 0 1 0-1.32l8.6-5.8A.8.8 0 0 1 12 6.2v5.14l8.25-5.8a.8.8 0 0 1 1.25.66z"/></svg>',
  left: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5l-7 7 7 7"/></svg>',
  right: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M9 5l7 7-7 7"/></svg>',
  upk: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5 15l7-7 7 7"/></svg>',
  down: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5 9l7 7 7-7"/></svg>',
  del: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"><path d="M9 5h11v14H9l-6-7z"/><path d="m12.5 9.5 5 5M17.5 9.5l-5 5"/></svg>',
  return: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"><path d="M19 5v6a3 3 0 0 1-3 3H5"/><path d="m9 10-4 4 4 4"/></svg>',
  moon: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M20.2 14.6A8.6 8.6 0 0 1 9.4 3.8a8.6 8.6 0 1 0 10.8 10.8z"/></svg>',
  lock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><rect x="5" y="10.5" width="14" height="10" rx="2.6" fill="currentColor" stroke="none"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5"/></svg>',
  power: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 3v8"/><path d="M6.6 6.6a7.6 7.6 0 1 0 10.8 0"/></svg>',
  plus: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>',
  share: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 15V3M8 7l4-4 4 4"/><path d="M6 11H5v10h14V11h-1"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>',
  copy: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linejoin="round"><rect x="8" y="8" width="12" height="12" rx="2.5"/><path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8"/></svg>',
  image: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="9" cy="10" r="2"/><path d="m4 18 5-5 4 4 3-3 5 5"/></svg>',
};
function fillIcons(root = document) {
  for (const e of root.querySelectorAll("[data-icon]")) if (!e.firstChild) e.innerHTML = ICON[e.dataset.icon] || "";
}

// ------------------------------------------------------------------ little helpers

function el(tag, cls, text) { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
function size(n) {
  if (n == null) return "";
  if (n < 1024) return n + " B";
  if (n < 1048576) return Math.round(n / 1024) + " KB";
  if (n < 1073741824) return (n / 1048576).toFixed(1) + " MB";
  return (n / 1073741824).toFixed(2) + " GB";
}
function ext(name) { const m = /\.([a-z0-9]{1,4})$/i.exec(name || ""); return m ? m[1].toUpperCase() : "FILE"; }
function ago(sec) {
  const d = Date.now() / 1000 - sec;
  if (d < 60) return "just now";
  if (d < 3600) return Math.round(d / 60) + " min ago";
  if (d < 86400) return Math.round(d / 3600) + " h ago";
  return Math.round(d / 86400) + " d ago";
}
function buzz(ms = 8) { try { navigator.vibrate && navigator.vibrate(ms); } catch (_) {} }
let toastTimer;
function toast(t) {
  const e = $("toast");
  e.textContent = t;
  e.classList.add("on");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => e.classList.remove("on"), 2600);
}
function ringSvg(cls = "ring") {
  const ns = "http://www.w3.org/2000/svg", s = document.createElementNS(ns, "svg");
  s.setAttribute("viewBox", "0 0 30 30");
  s.setAttribute("class", cls);
  for (const c of ["t", "v"]) {
    const k = document.createElementNS(ns, "circle");
    k.setAttribute("cx", 15); k.setAttribute("cy", 15); k.setAttribute("r", 12); k.setAttribute("class", c);
    s.append(k);
  }
  const len = 2 * Math.PI * 12, v = s.lastChild;
  v.style.strokeDasharray = len; v.style.strokeDashoffset = len;
  s.set = (f) => { v.style.strokeDashoffset = len * (1 - Math.max(0.02, Math.min(1, f))); };
  return s;
}
const store = {
  get(k, d) { try { const v = localStorage.getItem("openhop." + k); return v == null ? d : JSON.parse(v); } catch (_) { return d; } },
  set(k, v) { try { localStorage.setItem("openhop." + k, JSON.stringify(v)); } catch (_) {} },
};

// ------------------------------------------------------------------ bytes and keys

function b64url(b) {
  let s = "";
  for (let i = 0; i < b.length; i++) s += String.fromCharCode(b[i]);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function unb64url(s) {
  s = s.replace(/-/g, "+").replace(/_/g, "/");
  while (s.length % 4) s += "=";
  const r = atob(s), out = new Uint8Array(r.length);
  for (let i = 0; i < r.length; i++) out[i] = r.charCodeAt(i);
  return out;
}
const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
function rand(n) { const b = new Uint8Array(n); crypto.getRandomValues(b); return b; }
function cat(...parts) {
  const n = parts.reduce((a, p) => a + p.length, 0), out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}

const subtle = window.crypto && window.crypto.subtle;
async function hkdf(secret, salt, info) {
  const k = await subtle.importKey("raw", secret, "HKDF", false, ["deriveBits"]);
  return new Uint8Array(await subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt, info: enc.encode(info) }, k, 256));
}
async function aesKey(raw) { return subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt", "decrypt"]); }

/** What a paired computer's secret gives: its topic and the set-up key. */
async function keysFor(pc) {
  if (pc.keys) return pc.keys;
  const secret = unb64url(pc.secret), salt = enc.encode("openhop-phone");
  const topic = hex((await hkdf(secret, salt, "topic")).slice(0, 16));
  const key = await aesKey(await hkdf(secret, salt, "signal"));
  pc.keys = { secret, topic, key };
  return pc.keys;
}
async function sealSignal(keys, msg) {
  const iv = rand(12);
  const c = new Uint8Array(await subtle.encrypt({ name: "AES-GCM", iv, additionalData: enc.encode(keys.topic) }, keys.key, enc.encode(JSON.stringify(msg))));
  return b64url(cat(iv, c));
}
async function openSignal(keys, content) {
  try {
    const d = unb64url(content);
    const p = await subtle.decrypt({ name: "AES-GCM", iv: d.slice(0, 12), additionalData: enc.encode(keys.topic) }, keys.key, d.slice(12));
    const m = JSON.parse(dec.decode(p));
    if (Math.abs(Date.now() / 1000 - m.ts) > 120) return null;
    return m;
  } catch (_) { return null; }
}

/** The second layer inside a direct connection (the computer's side is crypto::Session). */
class Session {
  constructor(key) { this.key = key; this.sent = 0; this.got = 0; }
  static async make(secret, phoneNonce, pcNonce) { return new Session(await aesKey(await hkdf(secret, cat(phoneNonce, pcNonce), "data"))); }
  nonce(dir, n) { const v = new Uint8Array(12); v[0] = dir; new DataView(v.buffer).setBigUint64(4, BigInt(n)); return v; }
  async seal(m) { this.sent += 1; return new Uint8Array(await subtle.encrypt({ name: "AES-GCM", iv: this.nonce(1, this.sent) }, this.key, m)); }
  async open(m) { this.got += 1; return new Uint8Array(await subtle.decrypt({ name: "AES-GCM", iv: this.nonce(2, this.got) }, this.key, m)); }
}

// ------------------------------------------------------------------ meeting point (Nostr relays)

let secp = null;
async function nostrKeys() {
  if (!secp) secp = await import("./vendor/secp256k1.js");
  if (!nostrKeys.k) {
    let sk;
    do { sk = rand(32); } while (sk[0] === 0xff);
    nostrKeys.k = { sk, pk: hex(secp.schnorr.getPublicKey(sk)) };
  }
  return nostrKeys.k;
}

class Relays {
  constructor(urls) {
    this.urls = urls; this.socks = new Map(); this.topics = new Map(); this.seen = new Set(); this.up = 0;
    for (const u of urls) this.open(u, 1);
  }
  open(url, backoff) {
    let ws;
    try { ws = new WebSocket(url); } catch (_) { return; }
    this.socks.set(url, ws);
    ws.onopen = () => { this.up++; backoff = 1; for (const t of this.topics.keys()) this.req(ws, t); };
    ws.onmessage = (e) => this.onMsg(e.data);
    ws.onclose = () => {
      if (ws._up) this.up--;
      this.socks.delete(url);
      setTimeout(() => this.open(url, Math.min(backoff * 2, 30)), backoff * 1000);
    };
    ws.addEventListener("open", () => { ws._up = true; });
  }
  req(ws, topic) {
    try { ws.send(JSON.stringify(["REQ", "oh-" + topic.slice(0, 8), { kinds: [KIND], "#t": [topic], since: Math.floor(Date.now() / 1000) - 30 }])); } catch (_) {}
  }
  /** Listen to a computer's topic. */
  async watch(pc, onMsg) {
    const keys = await keysFor(pc);
    this.topics.set(keys.topic, { keys, onMsg });
    for (const ws of this.socks.values()) if (ws.readyState === 1) this.req(ws, keys.topic);
  }
  async onMsg(data) {
    let a;
    try { a = JSON.parse(data); } catch (_) { return; }
    if (a[0] !== "EVENT" || !a[2]) return;
    const ev = a[2];
    if (this.seen.has(ev.id)) return;
    this.seen.add(ev.id);
    const me = await nostrKeys();
    if (ev.pubkey === me.pk) return;
    const tag = (ev.tags || []).find((t) => t[0] === "t");
    const t = tag && this.topics.get(tag[1]);
    if (!t) return;
    const m = await openSignal(t.keys, ev.content);
    if (m) t.onMsg(m);
  }
  async send(pc, msg) {
    const keys = await keysFor(pc), me = await nostrKeys();
    msg.ts = Math.floor(Date.now() / 1000);
    const content = await sealSignal(keys, msg);
    const created_at = msg.ts, tags = [["t", keys.topic]];
    const id = new Uint8Array(await subtle.digest("SHA-256", enc.encode(JSON.stringify([0, me.pk, created_at, KIND, tags, content]))));
    const sig = hex(await secp.schnorr.signAsync(id, me.sk));
    const ev = { id: hex(id), pubkey: me.pk, created_at, kind: KIND, tags, content, sig };
    const text = JSON.stringify(["EVENT", ev]);
    for (const ws of this.socks.values()) if (ws.readyState === 1) { try { ws.send(text); } catch (_) {} }
  }
}

// ------------------------------------------------------------------ links to a computer

/** Shared parts: requests by number, pushes to listeners. */
class Link {
  constructor() { this.rid = 1; this.waiting = new Map(); this.onPush = () => {}; this.onClose = () => {}; this.closed = false; }
  close() { this.closed = true; }
}

class DirectLink extends Link {
  constructor(pc, relays) { super(); this.pc = pc; this.relays = relays; this.kind = "direct"; this.x = 1; this.uploads = new Map(); this.incoming = new Map(); this.chain = Promise.resolve(); this.inChain = Promise.resolve(); }

  async connect() {
    const keys = await keysFor(this.pc);
    const rtc = new RTCPeerConnection({ iceServers: STUN });
    this.rtc = rtc;
    const dc = rtc.createDataChannel("openhop", { ordered: true });
    dc.binaryType = "arraybuffer";
    dc.bufferedAmountLowThreshold = 1 << 20;
    this.dc = dc;
    await rtc.setLocalDescription(await rtc.createOffer());
    await new Promise((res) => {
      if (rtc.iceGatheringState === "complete") return res();
      const t = setTimeout(res, 2500);
      rtc.addEventListener("icegatheringstatechange", () => { if (rtc.iceGatheringState === "complete") { clearTimeout(t); res(); } });
    });
    const nonce = rand(16), sid = hex(rand(8));
    const answer = await new Promise((res, rej) => {
      const t = setTimeout(() => rej(new Error("no answer")), 12000);
      this.pc.onSignal = (m) => { if (m.t === "answer" && m.sid === sid) { clearTimeout(t); res(m); } };
      this.relays.send(this.pc, { t: "offer", to: this.pc.id, from: me(), sid, sdp: rtc.localDescription.sdp, n: b64url(nonce), device: DEVICE });
    });
    await rtc.setRemoteDescription({ type: "answer", sdp: answer.sdp });
    this.session = await Session.make(keys.secret, nonce, unb64url(answer.n));
    await new Promise((res, rej) => {
      const t = setTimeout(() => rej(new Error("couldn't connect")), 15000);
      dc.onopen = () => { clearTimeout(t); res(); };
    });
    dc.onmessage = (e) => { const d = new Uint8Array(e.data); this.inChain = this.inChain.then(() => this.onData(d)).catch(() => this.drop()); };
    dc.onclose = () => this.drop();
    rtc.addEventListener("connectionstatechange", () => { if (["failed", "closed", "disconnected"].includes(rtc.connectionState)) this.drop(); });
  }
  drop() { if (this.closed) return; this.closed = true; try { this.rtc.close(); } catch (_) {} this.onClose(); }
  close() { this.drop(); }

  async onData(d) {
    const p = await this.session.open(d);
    if (p[0] === 1) {
      const m = JSON.parse(dec.decode(p.subarray(1)));
      if (m.re != null) { const w = this.waiting.get(m.re); if (w) { this.waiting.delete(m.re); w(m); } return; }
      if (m.push === "put_done") { const u = this.uploads.get(m.x); if (u) { this.uploads.delete(m.x); u(m); } return; }
      if (m.push === "file") { this.incoming.set(m.x, { ...m, parts: [], got: 0 }); this.onPush({ push: "file_start", file: this.incoming.get(m.x) }); return; }
      if (m.push === "file_end") {
        const f = this.incoming.get(m.x);
        this.incoming.delete(m.x);
        if (f) this.onPush({ push: "file_done", file: f, ok: m.ok, blob: m.ok ? new Blob(f.parts, { type: f.mime }) : null });
        return;
      }
      this.onPush(m);
    } else if (p[0] === 2) {
      const x = new DataView(p.buffer, p.byteOffset).getUint32(1);
      const f = this.incoming.get(x);
      if (!f) return;
      f.parts.push(p.slice(5));
      f.got += p.length - 5;
      this.onPush({ push: "file_progress", file: f });
    }
  }
  /** Seal and send, in order, never far ahead of the connection. */
  frame(kind, body) {
    const job = this.chain.then(async () => {
      if (this.closed) throw new Error("closed");
      const c = await this.session.seal(cat(new Uint8Array([kind]), body));
      while (this.dc.bufferedAmount > (4 << 20)) await new Promise((r) => { this.dc.onbufferedamountlow = r; setTimeout(r, 200); });
      this.dc.send(c);
    });
    this.chain = job.catch(() => {});
    return job;
  }
  request(op) {
    const rid = this.rid++;
    return new Promise((res, rej) => {
      const t = setTimeout(() => { this.waiting.delete(rid); rej(new Error("no reply")); }, 15000);
      this.waiting.set(rid, (m) => { clearTimeout(t); res(m); });
      this.frame(1, enc.encode(JSON.stringify({ ...op, rid }))).catch(rej);
    });
  }
  fire(op) { this.frame(1, enc.encode(JSON.stringify(op))).catch(() => {}); }
  async upload(file, to, progress) {
    const x = this.x++;
    const done = new Promise((res) => this.uploads.set(x, res));
    const r = await this.request({ op: "put", x, name: file.name, size: file.size, to });
    if (!r.ok) throw new Error(r.error || "refused");
    const head = new Uint8Array(4);
    new DataView(head.buffer).setUint32(0, x);
    for (let off = 0; off < file.size; off += CHUNK) {
      const buf = new Uint8Array(await file.slice(off, off + CHUNK).arrayBuffer());
      await this.frame(2, cat(head, buf));
      progress(Math.max(0, off + buf.length - this.dc.bufferedAmount) / file.size);
    }
    const m = await done;
    if (!m.ok) throw new Error("didn't arrive");
    return m.name;
  }
  get(id) { return this.request({ op: "get", id }); }
}

class LanLink extends Link {
  constructor() { super(); this.kind = "local"; this.base = location.pathname; this.cid = store.get("cid") || hex(rand(8)); store.set("cid", this.cid); }
  async connect() { const r = await this.request({ op: "hello", device: DEVICE }); if (!r.ok) throw new Error("no"); return r; }
  async request(op) {
    const r = await fetch(this.base + "op", { method: "POST", body: JSON.stringify({ ...op, cid: this.cid }), headers: { "Content-Type": "application/json" } });
    if (!r.ok) throw new Error("HTTP " + r.status);
    return r.json();
  }
  fire(op) { this.request(op).catch(() => {}); }
  upload(file, to, progress) {
    return new Promise((res, rej) => {
      const x = Math.floor(Math.random() * 1e9);
      const q = `up?x=${x}&name=${encodeURIComponent(file.name)}&to=${encodeURIComponent(to)}&cid=${this.cid}`;
      const req = new XMLHttpRequest();
      req.open("PUT", this.base + q);
      req.upload.onprogress = (e) => { if (e.lengthComputable) progress(e.loaded / e.total); };
      req.onload = () => { try { const m = JSON.parse(req.responseText); m.ok ? res(m.name) : rej(new Error(m.error || "failed")); } catch (e) { rej(e); } };
      req.onerror = () => rej(new Error("network"));
      req.send(file);
    });
  }
  /** Same-Wi-Fi: the phone fetches offered files itself. */
  async get(id, meta) {
    const f = { x: id, id, name: meta.name, size: meta.size, mime: "", parts: [], got: 0 };
    this.onPush({ push: "file_start", file: f });
    const r = await fetch(this.base + "dl/" + id);
    f.mime = r.headers.get("Content-Type") || "";
    const reader = r.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      f.parts.push(value); f.got += value.length;
      this.onPush({ push: "file_progress", file: f });
    }
    this.onPush({ push: "file_done", file: f, ok: true, blob: new Blob(f.parts, { type: f.mime }) });
    this.fire({ op: "got", id });
    return { ok: true };
  }
}

// ------------------------------------------------------------------ paired computers

function me() { let id = store.get("me"); if (!id) { id = hex(rand(8)); store.set("me", id); } return id; }
function pcs() { return store.get("pcs", []); }
function savePcs(list) { store.set("pcs", list.map(({ id, secret, name, lan, relays }) => ({ id, secret, name, lan, relays }))); }
/** Every meeting point any paired computer uses. */
function relayList() {
  const set = new Set();
  for (const p of pcs()) for (const u of p.relays && p.relays.length ? p.relays : RELAYS) set.add(u);
  return set.size ? Array.from(set) : RELAYS;
}

/** "#p=<id>.<secret>.<name>.<same-Wi-Fi address>" from the QR code. */
function parsePair(text) {
  const m = /[#&]p=([0-9a-f]{16})\.([A-Za-z0-9_-]{43})\.([A-Za-z0-9_-]*)\.([A-Za-z0-9_-]*)/.exec(text || "");
  if (!m) return null;
  let name = "Computer", lan = "";
  try { name = dec.decode(unb64url(m[3])) || name; } catch (_) {}
  try { lan = dec.decode(unb64url(m[4])); } catch (_) {}
  // A school's or company's own meeting points.
  const r = /[#&]r=([A-Za-z0-9_-]+)/.exec(text);
  let relays;
  try { if (r) relays = dec.decode(unb64url(r[1])).split(/\s+/).filter((u) => /^wss?:\/\//.test(u)); } catch (_) {}
  return { id: m[1], secret: m[2], name, lan, relays };
}
function addPc(p) {
  const list = pcs().filter((x) => x.id !== p.id);
  list.unshift(p);
  savePcs(list);
  store.set("active", p.id);
}

// ------------------------------------------------------------------ the app

const app = {
  relays: null, link: null, pc: null, state: null, online: new Map(), connecting: false, tries: 0,
  to: store.get("to", ""), feed: [], has: new Set(store.get("has", [])), tab: "send",
};

function activePc() {
  if (LAN) return app.pc || { id: "lan", name: "Computer", lan: location.host + location.pathname };
  const list = pcs();
  return list.find((p) => p.id === store.get("active")) || list[0] || null;
}

async function start() {
  fillIcons();
  // Paired from the QR code.
  const p = parsePair(location.hash);
  if (p && !LAN) addPc(p);
  if (!LAN && pcs().length === 0) { showWelcome(); return; }
  $("welcome").hidden = true;
  $("main").hidden = false;
  $("tabs").hidden = false;
  if (!LAN) {
    app.relays = new Relays(relayList());
    for (const pc of pcs()) watch(pc);
    hiAll();
    setInterval(() => { if (!document.hidden) hiAll(); }, 20000);
  }
  await loadFeed();
  connect();
  setInterval(refresh, 2500);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) { if (!app.link) connect(); else refresh(); } });
}

function watch(pc) {
  app.relays.watch(pc, (m) => {
    if (m.t === "here" && m.to === me()) { app.online.set(pc.id, Date.now()); if (pc.name !== m.name && m.name) { pc.name = m.name; savePcs(pcs().map((x) => (x.id === pc.id ? { ...x, name: m.name } : x))); } renderPcs(); renderNotch(); }
    if (pc.onSignal) pc.onSignal(m);
  });
}
function hiAll() { for (const pc of pcs()) app.relays.send(pc, { t: "hi", from: me() }).catch(() => {}); }

async function connect() {
  if (app.connecting) return;
  const pc = activePc();
  if (!pc) return;
  app.connecting = true;
  app.pc = pc;
  renderNotch();
  try {
    let link;
    if (LAN) {
      link = new LanLink();
      const hello = await link.connect();
      pc.name = hello.me;
    } else {
      // Its own object: the relay listener calls back on it.
      const live = pcs().find((x) => x.id === pc.id) || pc;
      const keys = await keysFor(live);
      Object.assign(pc, { keys });
      watch(pc);
      link = new DirectLink(pc, app.relays);
      await link.connect();
      await link.request({ op: "hello", device: DEVICE });
    }
    if (app.link) app.link.close();
    app.link = link;
    app.tries = 0;
    link.onPush = onPush;
    link.onClose = () => { if (app.link === link) { app.link = null; renderNotch(); setTimeout(connect, 1500); } };
    buzz(12);
    await refresh();
  } catch (e) {
    app.tries++;
    console.warn("connect:", e);
    const wait = Math.min(30000, 2000 * app.tries);
    setTimeout(() => { if (!app.link && !document.hidden) connect(); }, wait);
  } finally {
    app.connecting = false;
    renderNotch();
  }
}

async function refresh() {
  if (!app.link || document.hidden) return;
  try {
    const s = await app.link.request({ op: "state" });
    if (!s.ok) return;
    app.state = s;
    if (LAN) app.pc.name = s.me;
    // Files the computer offered that this phone doesn't have yet.
    for (const o of s.offered || []) {
      const key = `${app.pc.id}:${o.name}:${o.size}`;
      if (app.has.has(key) || app.getting === key) continue;
      if (LAN) { app.getting = key; app.link.get(o.id, o).catch(() => {}).finally(() => { app.getting = null; }); break; }
      else if (!(s.has || []).includes(o.id)) app.link.get(o.id).catch(() => {});
    }
    render();
  } catch (_) {}
}

// ------------------------------------------------------------------ notch

let notchBusy = null;
function renderNotch() {
  const pc = app.pc || activePc();
  if (notchBusy) return;
  $("notch").classList.remove("big");
  $("nRing").hidden = true;
  const dot = $("nDot"), text = $("nText");
  const name = pc ? pc.name : "OpenHop";
  if (app.link) {
    dot.className = "n-dot on";
    text.innerHTML = "";
    text.append(name);
  } else if (app.connecting || (pc && app.online.has(pc.id))) {
    dot.className = "n-dot wait";
    text.textContent = `Connecting to ${name}…`;
  } else {
    dot.className = "n-dot";
    text.textContent = pc ? `${name} isn't reachable` : "OpenHop";
  }
}
/** The notch grows into a live activity: a ring and two lines. */
function notchActivity(title, sub) {
  notchBusy = { title, sub };
  const n = $("notch");
  n.classList.add("big");
  $("nDot").className = "n-dot on";
  const t = $("nText");
  t.innerHTML = "";
  t.append(title);
  const s = el("small", null, sub);
  t.append(s);
  const r = $("nRing");
  r.hidden = false;
  if (!r.firstChild) r.append(ringSvg("ringn"));
  r.classList.remove("done");
  const ring = r.querySelector("svg");
  return {
    set: (f) => ring.set(f),
    done(label) {
      ring.set(1);
      r.classList.add("done");
      const ns = "http://www.w3.org/2000/svg", p = document.createElementNS(ns, "path");
      p.setAttribute("d", "M9.5 15.5l3.6 3.6 7.4-7.6"); p.setAttribute("class", "tick");
      ring.append(p);
      t.firstChild.textContent = label;
      buzz(18);
      setTimeout(() => { p.remove(); notchBusy = null; renderNotch(); }, 1700);
    },
    fail(label) { t.firstChild.textContent = label; setTimeout(() => { notchBusy = null; renderNotch(); }, 2200); },
  };
}
/** A file flies from `from` (an element) into the notch. */
function fly(from, file) {
  const r = from.getBoundingClientRect(), n = $("notch").getBoundingClientRect();
  const g = el("div", "ghost", ext(file && file.name));
  if (file && /^image\//.test(file.type)) { g.textContent = ""; g.style.backgroundImage = `url(${URL.createObjectURL(file)})`; }
  g.style.left = r.left + r.width / 2 - 28 + "px";
  g.style.top = r.top + r.height / 2 - 28 + "px";
  document.body.append(g);
  const dx = n.left + n.width / 2 - (r.left + r.width / 2), dy = n.top + n.height / 2 - (r.top + r.height / 2);
  g.animate([{ transform: "translate(0,0) scale(1)", opacity: 1 }, { transform: `translate(${dx * 0.55}px, ${dy * 0.55 - 40}px) scale(.8)`, opacity: 1, offset: 0.55 },
    { transform: `translate(${dx}px, ${dy}px) scale(.2)`, opacity: 0 }], { duration: 620, easing: "cubic-bezier(.4,0,.2,1)" }).onfinish = () => g.remove();
}

// ------------------------------------------------------------------ sending

function targetName() {
  if (!app.to) return app.pc ? app.pc.name : "your computer";
  if (app.to === "*") return "all your computers";
  if (app.to === "shelf") return "the shelf";
  return app.to;
}
async function sendFiles(files, from) {
  if (!files.length) return;
  if (!app.link) { toast("Not connected yet. Hold on a moment."); connect(); return; }
  buzz(10);
  files.slice(0, 4).forEach((f, i) => setTimeout(() => fly(from, f), i * 90));
  const to = app.to, where = targetName();
  const total = files.reduce((a, f) => a + f.size, 0);
  let doneBytes = 0, ok = 0;
  const label = files.length === 1 ? files[0].name : `${files.length} items`;
  const act = notchActivity(`Sending ${label}`, `to ${where}`);
  for (const f of files) {
    const item = addItem({ name: f.name, size: f.size, type: f.type, file: f, out: true, sub: `to ${where}` });
    try {
      await app.link.upload(f, to, (p) => { item.ring.set(p); act.set((doneBytes + p * f.size) / Math.max(1, total)); });
      ok++;
      item.finish(true);
    } catch (e) {
      item.finish(false, e.message);
    }
    doneBytes += f.size;
  }
  if (ok === files.length) act.done(files.length === 1 ? `Sent to ${where}` : `${ok} sent to ${where}`);
  else act.fail(ok ? `${ok} of ${files.length} sent` : "Didn't send");
  refresh();
}

// ------------------------------------------------------------------ receiving

let liveIn = null;
function onPush(m) {
  if (m.push === "file_start") {
    const f = m.file;
    f.item = addItem({ name: f.name, size: f.size, type: f.mime, out: false, sub: `from ${app.pc.name}` });
    liveIn = notchActivity(`Receiving ${f.name}`, `from ${app.pc.name}`);
  } else if (m.push === "file_progress") {
    const f = m.file;
    if (f.item) f.item.ring.set(f.got / Math.max(1, f.size));
    if (liveIn) liveIn.set(f.got / Math.max(1, f.size));
  } else if (m.push === "file_done") {
    const f = m.file;
    app.has.add(`${app.pc.id}:${f.name}:${f.size}`);
    store.set("has", Array.from(app.has).slice(-300));
    if (!m.ok || !m.blob) { if (f.item) f.item.finish(false); if (liveIn) liveIn.fail("Didn't arrive"); return; }
    const file = new File([m.blob], f.name, { type: m.blob.type || f.mime || "" });
    if (f.item) f.item.received(file);
    if (liveIn) liveIn.done(`${f.name} is here`);
    saveReceived(file);
    if (store.get("autosave", !IOS)) download(file);
  }
}

async function save(file) {
  // iPhone: the share sheet has Save Image / Save to Files.
  if (navigator.canShare && navigator.canShare({ files: [file] })) {
    try { await navigator.share({ files: [file] }); return; } catch (e) { if (e.name === "AbortError") return; }
  }
  download(file);
}
function download(file) {
  const a = el("a");
  a.href = URL.createObjectURL(file);
  a.download = file.name;
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 4000);
}

// Received files are kept on the phone (until 40 newer ones arrive).
let db = null;
function idb() {
  if (db) return db;
  db = new Promise((res) => {
    try {
      const r = indexedDB.open("openhop", 1);
      r.onupgradeneeded = () => r.result.createObjectStore("files", { keyPath: "at" });
      r.onsuccess = () => res(r.result);
      r.onerror = () => res(null);
    } catch (_) { res(null); }
  });
  return db;
}
async function saveReceived(file) {
  const d = await idb();
  if (!d) return;
  try {
    const tx = d.transaction("files", "readwrite"), s = tx.objectStore("files");
    s.put({ at: Date.now(), name: file.name, type: file.type, blob: file, from: app.pc ? app.pc.name : "" });
    const keys = s.getAllKeys();
    keys.onsuccess = () => { const k = keys.result; for (const old of k.slice(0, Math.max(0, k.length - 40))) s.delete(old); };
  } catch (_) {}
}
async function loadFeed() {
  const d = await idb();
  if (!d) return;
  await new Promise((res) => {
    try {
      const r = d.transaction("files").objectStore("files").getAll();
      r.onsuccess = () => {
        for (const x of r.result.slice(-12)) {
          const file = new File([x.blob], x.name, { type: x.type });
          const it = addItem({ name: x.name, size: file.size, type: x.type, out: false, sub: `from ${x.from}`, quiet: true });
          it.received(file, true);
        }
        res();
      };
      r.onerror = res;
    } catch (_) { res(); }
  });
}

// ------------------------------------------------------------------ the feed

function addItem({ name, size: n, type, file, out, sub, quiet }) {
  const row = el("div", "item" + (quiet ? "" : " new"));
  const th = el("div", "thumb", ext(name));
  if (file && /^image\//.test(type)) { th.textContent = ""; th.style.backgroundImage = `url(${URL.createObjectURL(file)})`; }
  const meta = el("div", "i-meta");
  meta.append(el("b", null, name), el("span", null, `${size(n)} · ${sub}`));
  const act = el("div", "i-act");
  const ring = ringSvg();
  act.append(ring);
  row.append(th, meta, act);
  $("feed").prepend(row);
  $("feedEmpty").hidden = true;
  const it = {
    row, ring, file: null,
    finish(ok, why) {
      act.innerHTML = "";
      if (ok) { const c = el("span", "ok"); c.innerHTML = ICON.check; act.append(c); }
      else act.append(el("span", "bad", why && why.length < 30 ? why : "Didn't go"));
    },
    received(f, old) {
      it.file = f;
      if (/^image\//.test(f.type)) { th.textContent = ""; th.style.backgroundImage = `url(${URL.createObjectURL(f)})`; }
      act.innerHTML = "";
      const b = el("button", "pill" + (old ? " soft" : ""), "Save");
      b.addEventListener("click", () => save(f));
      act.append(b);
      th.addEventListener("click", () => window.open(URL.createObjectURL(f), "_blank"));
      app.feed.push(it);
      renderSaveAll();
    },
  };
  return it;
}
function renderSaveAll() { $("saveAll").hidden = app.feed.filter((i) => i.file).length < 2; }
$("saveAll").addEventListener("click", async () => {
  const files = app.feed.filter((i) => i.file).map((i) => i.file).slice(-20);
  if (navigator.canShare && navigator.canShare({ files })) { try { await navigator.share({ files }); return; } catch (e) { if (e.name === "AbortError") return; } }
  for (const f of files) download(f);
});

// ------------------------------------------------------------------ rendering

function render() {
  renderNotch();
  renderTargets();
  renderOnward();
  if (app.tab === "shelf") renderShelf();
  if (app.tab === "clips") renderClips();
  if (app.tab === "remote") renderRemote();
}

/** Files on their way from the computer to the others (sent to "All" or another computer). */
function renderOnward() {
  const box = $("onward");
  const list = ((app.state && app.state.transfers) || []).filter((t) => !t.incoming && t.peer !== DEVICE && !/phone|iPhone|iPad/.test(t.peer));
  box.innerHTML = "";
  for (const t of list.slice(0, 6)) {
    const r = el("div", "hop"), ring = ringSvg();
    ring.set(t.total ? t.done / t.total : 0);
    r.append(ring, el("b", null, t.label), el("span", null, `→ ${t.peer}`));
    box.append(r);
  }
}

let targetsKey = "";
function renderTargets() {
  const s = app.state;
  const others = ((s && s.pcs) || []).filter((p) => !p.this);
  const key = JSON.stringify([app.to, app.pc && app.pc.name, others.map((p) => p.name)]);
  if (key === targetsKey) return;
  targetsKey = key;
  const row = $("toRow");
  row.innerHTML = "";
  const opts = [{ v: "", label: app.pc ? app.pc.name : "Computer", icon: "laptop", dot: !!app.link }];
  for (const p of others) opts.push({ v: p.name, label: p.name, icon: "laptop" });
  if (others.length) opts.push({ v: "*", label: "All computers", icon: "all" });
  opts.push({ v: "shelf", label: "Shelf", icon: "tray" });
  if (!opts.some((o) => o.v === app.to)) app.to = "";
  for (const o of opts) {
    const b = el("button", "chip");
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(o.v === app.to));
    const ic = el("span"); ic.dataset.icon = o.icon;
    b.append(ic, o.label);
    if (o.dot != null) { const d = el("i", "dot" + (o.dot ? "" : " off")); b.prepend(d); }
    b.addEventListener("click", () => { app.to = o.v; store.set("to", o.v); buzz(5); targetsKey = ""; renderTargets(); });
    row.append(b);
  }
  fillIcons(row);
}

function renderShelf() {
  const box = $("shelfList");
  box.innerHTML = "";
  const items = (app.state && app.state.shelf) || [];
  if (!items.length) { box.append(el("p", "empty", "Nothing on the shelf. Drop files on the island on any computer to keep them here.")); return; }
  const by = new Map();
  for (const i of items) { if (!by.has(i.origin)) by.set(i.origin, []); by.get(i.origin).push(i); }
  for (const [origin, list] of by) {
    box.append(el("div", "group-title", origin));
    const l = el("div", "list");
    for (const i of list) {
      const r = el("button", "row");
      const ic = el("span", "ic"); const ii = el("span"); ii.dataset.icon = i.count > 1 ? "tray" : "file"; ic.append(ii);
      const m = el("span", "r-meta"); m.append(el("b", null, i.label), el("span", null, `${size(i.size)}${i.count > 1 ? ` · ${i.count} files` : ""}`));
      r.append(ic, m, el("span", "r-tail", "Get"));
      r.addEventListener("click", async () => {
        buzz();
        const res = await app.link.request({ op: "shelf_get", origin, id: i.id }).catch(() => null);
        toast(res && res.ok ? (res.later ? `Fetching from ${origin}…` : "Coming to your phone…") : "Couldn't get it");
        setTimeout(refresh, 600);
      });
      l.append(r);
    }
    box.append(l);
  }
  fillIcons(box);
}

function renderClips() {
  const box = $("clipList");
  const q = $("clipQ").value.trim().toLowerCase();
  const items = ((app.state && app.state.clips) || []).filter((c) => !q || (c.text || "").toLowerCase().includes(q));
  const key = JSON.stringify([q, items.map((c) => c.id)]);
  if (box.dataset.key === key) return;
  box.dataset.key = key;
  box.innerHTML = "";
  $("pasteToPcL").textContent = `Send what's copied here to ${app.pc ? app.pc.name : "the computer"}`;
  for (const c of items) {
    const r = el("div", "row");
    const ic = el("span", "ic");
    if (c.thumb) ic.style.backgroundImage = `url(${c.thumb})`;
    else { const ii = el("span"); ii.dataset.icon = c.kind === "files" ? "file" : c.kind === "image" ? "image" : "text"; ic.append(ii); }
    const m = el("span", "r-meta");
    m.append(el("b", "clip", c.text), el("span", null, `${c.from} · ${ago(c.at)}`));
    const copy = el("button", "r-tail", c.kind === "text" ? "Copy" : "Use");
    copy.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (c.kind === "text") {
        try { await navigator.clipboard.writeText(c.text); toast("Copied on this phone"); buzz(); } catch (_) { toast("Couldn't copy here"); }
      } else {
        app.link && app.link.fire({ op: "clip_use", id: c.id });
        toast(`Copied again on ${app.pc.name}`);
      }
    });
    r.addEventListener("click", () => { app.link && app.link.fire({ op: "clip_use", id: c.id }); toast(`Copied again on ${app.pc.name}`); buzz(); });
    r.append(ic, m, copy);
    box.append(r);
  }
  if (!items.length) box.append(el("p", "empty", q ? "Nothing matches." : "Things you copy on your computers show up here."));
  fillIcons(box);
}

let sleepArmed = 0;
function renderRemote() {
  const s = app.state || {};
  $("padPc").textContent = app.pc ? app.pc.name : "the computer";
  const m = (s.media || []).find((x) => x.now.playing) || (s.media || [])[0];
  $("player").hidden = !m;
  if (m) {
    $("pTitle").textContent = m.now.title || m.now.app;
    $("pArtist").textContent = m.now.artist || "";
    $("pWhere").textContent = `${m.now.app} on ${m.name}`;
    $("pArt").style.backgroundImage = m.now.art ? `url(${m.now.art})` : "";
    $("pPlay").innerHTML = m.now.playing ? ICON.pause : ICON.play;
    $("player").dataset.on = m.name;
  }
  const f = s.features || {};
  $("qFocus").hidden = f.focus === false;
  $("qLock").hidden = f.lock === false;
  $("qSleep").hidden = f.sleep === false;
  $("qFocus").classList.toggle("on", !!s.focus);
  $("qSleep").classList.toggle("armed", Date.now() < sleepArmed);
  $("qSleep").lastChild.textContent = Date.now() < sleepArmed ? "Tap again" : "Sleep all";
}

function renderPcs() {
  const box = $("pcList");
  box.innerHTML = "";
  const list = LAN ? [activePc()] : pcs();
  for (const p of list) {
    const r = el("div", "row");
    const ic = el("span", "ic"); const ii = el("span"); ii.dataset.icon = "laptop"; ic.append(ii);
    const live = app.pc && app.pc.id === p.id && app.link;
    const seen = app.online.get(p.id);
    const m = el("span", "r-meta");
    m.append(el("b", null, p.name), el("span", null, live ? (app.link.kind === "direct" ? "Connected · end-to-end encrypted" : "Connected on this Wi‑Fi") : seen && Date.now() - seen < 60000 ? "Online" : "Not reachable right now"));
    r.append(ic, m);
    if (!LAN) {
      const go = el("button", "r-tail", live ? "" : "Connect");
      if (!live) go.addEventListener("click", () => { store.set("active", p.id); if (app.link) app.link.close(); app.link = null; app.state = null; targetsKey = ""; closeSheets(); connect(); });
      const forget = el("button", "r-tail", "Forget");
      forget.style.color = "var(--bad)";
      forget.addEventListener("click", () => { savePcs(pcs().filter((x) => x.id !== p.id)); if (app.pc && app.pc.id === p.id && app.link) app.link.close(); renderPcs(); if (!pcs().length) location.reload(); });
      r.append(go, forget);
    }
    box.append(r);
  }
  fillIcons(box);
  const how = $("howBox");
  const pc = activePc();
  how.innerHTML = "";
  if (LAN) how.textContent = "Connected through your Wi‑Fi. Open OpenHop from the code on the computer to connect from anywhere, encrypted end to end.";
  else if (app.link) how.textContent = "Connected directly to your computer, on any network, encrypted end to end. Nobody in between can read it.";
  else {
    how.textContent = "Can't reach your computer right now. It needs to be on, with OpenHop running.";
    if (pc && pc.lan) {
      const b = el("button", "plain", "Use the same‑Wi‑Fi connection instead");
      b.addEventListener("click", () => { location.href = `http://${pc.lan}/`; });
      how.append(b);
    }
  }
}

// ------------------------------------------------------------------ tabs and sheets

for (const b of $("tabs").querySelectorAll("button")) {
  b.addEventListener("click", () => {
    app.tab = b.dataset.go;
    for (const x of $("tabs").querySelectorAll("button")) x.toggleAttribute("aria-current", x === b), x === b && x.setAttribute("aria-current", "page");
    for (const s of document.querySelectorAll(".tab")) s.hidden = s.dataset.tab !== app.tab;
    window.scrollTo(0, 0);
    buzz(4);
    render();
  });
}
function openSheet(id) { $("scrim").hidden = false; $(id).hidden = false; }
function closeSheets() { $("scrim").hidden = true; for (const s of document.querySelectorAll(".sheet")) s.hidden = true; stopScan(); }
$("scrim").addEventListener("click", closeSheets);
$("notch").addEventListener("click", () => { renderPcs(); renderInstall(); openSheet("pcSheet"); });
$("addPc").addEventListener("click", () => { closeSheets(); scan(); });

// Send
$("pickPhotos").addEventListener("change", (e) => { const f = Array.from(e.target.files || []); e.target.value = ""; sendFiles(f, e.target.parentElement); });
$("pickFiles").addEventListener("change", (e) => { const f = Array.from(e.target.files || []); e.target.value = ""; sendFiles(f, e.target.parentElement); });
$("pickText").addEventListener("click", () => { openSheet("textSheet"); setTimeout(() => $("textIn").focus(), 300); });
$("textIn").addEventListener("input", () => { $("textOpen").hidden = !/^https?:\/\/\S+$/.test($("textIn").value.trim()); });
$("textCopy").addEventListener("click", async () => {
  const text = $("textIn").value;
  if (!text || !app.link) return;
  const r = await app.link.request({ op: "clip_set", text }).catch(() => null);
  toast(r && r.ok ? `Copied on ${app.pc.name}` : "Didn't send");
  if (r && r.ok) { $("textIn").value = ""; closeSheets(); buzz(12); }
});
$("textOpen").addEventListener("click", async () => {
  const url = $("textIn").value.trim();
  const r = app.link && (await app.link.request({ op: "open_url", url }).catch(() => null));
  toast(r && r.ok ? `Opened on ${app.pc.name}` : "Didn't open");
  if (r && r.ok) { $("textIn").value = ""; closeSheets(); }
});
// Share a file into the page (drag and drop on desktop browsers, too).
document.addEventListener("dragover", (e) => e.preventDefault());
document.addEventListener("drop", (e) => { e.preventDefault(); sendFiles(Array.from(e.dataTransfer.files || []), $("notch")); });

// Clipboard
$("clipQ").addEventListener("input", renderClips);
$("pasteToPc").addEventListener("click", async () => {
  let text = "";
  try { text = await navigator.clipboard.readText(); } catch (_) {}
  if (!text) { openSheet("textSheet"); toast("Paste it here, then tap Copy on computer"); return; }
  const r = await app.link.request({ op: "clip_set", text }).catch(() => null);
  toast(r && r.ok ? `Copied on ${app.pc.name}` : "Didn't send");
  buzz(10);
});

// Remote: music
$("pPlay").addEventListener("click", () => { app.link && app.link.fire({ op: "media", on: $("player").dataset.on, cmd: "toggle" }); buzz(); setTimeout(refresh, 400); });
$("pNext").addEventListener("click", () => { app.link && app.link.fire({ op: "media", on: $("player").dataset.on, cmd: "next" }); buzz(); setTimeout(refresh, 600); });
$("pPrev").addEventListener("click", () => { app.link && app.link.fire({ op: "media", on: $("player").dataset.on, cmd: "previous" }); buzz(); setTimeout(refresh, 600); });
// Remote: keys and typing
for (const b of document.querySelectorAll("[data-key]")) b.addEventListener("click", () => { app.link && app.link.fire({ op: "key", k: b.dataset.key }); buzz(6); });
$("typer").addEventListener("submit", (e) => {
  e.preventDefault();
  const t = $("typeIn").value;
  if (!app.link) return;
  if (t) app.link.fire({ op: "type", text: t });
  else app.link.fire({ op: "key", k: "enter" });
  $("typeIn").value = "";
  buzz(6);
});
$("qFocus").addEventListener("click", () => { app.link && app.link.fire({ op: "focus", on: !(app.state && app.state.focus) }); buzz(); setTimeout(refresh, 300); });
$("qLock").addEventListener("click", () => { app.link && app.link.fire({ op: "lock_all" }); toast("Locking every computer"); buzz(16); });
$("qSleep").addEventListener("click", () => {
  if (Date.now() < sleepArmed) { app.link && app.link.fire({ op: "sleep_all" }); sleepArmed = 0; toast("Putting every computer to sleep"); buzz(20); }
  else { sleepArmed = Date.now() + 3000; setTimeout(renderRemote, 3100); }
  renderRemote();
});
$("padL").addEventListener("click", () => { app.link && app.link.fire({ op: "click", b: "left" }); buzz(4); });
$("padR").addEventListener("click", () => { app.link && app.link.fire({ op: "click", b: "right" }); buzz(4); });

// Remote: trackpad. One finger moves (faster when you move faster), a tap
// clicks, two fingers scroll, a two-finger tap right-clicks.
(() => {
  const pad = $("pad");
  const pts = new Map();
  let acc = { dx: 0, dy: 0, sx: 0, sy: 0 }, raf = 0, startT = 0, moved = 0, maxFingers = 0;
  const flush = () => {
    raf = 0;
    if (!app.link) return;
    if (acc.dx || acc.dy) app.link.fire({ op: "mouse", dx: Math.round(acc.dx), dy: Math.round(acc.dy) });
    if (Math.abs(acc.sy) >= 1 || Math.abs(acc.sx) >= 1) app.link.fire({ op: "scroll", dx: Math.trunc(acc.sx), dy: Math.trunc(acc.sy) });
    acc.dx = acc.dy = 0;
    acc.sx -= Math.trunc(acc.sx); acc.sy -= Math.trunc(acc.sy);
  };
  pad.addEventListener("pointerdown", (e) => {
    pad.setPointerCapture(e.pointerId);
    pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pts.size === 1) { startT = Date.now(); moved = 0; maxFingers = 1; }
    maxFingers = Math.max(maxFingers, pts.size);
    pad.classList.add("touch");
  });
  pad.addEventListener("pointermove", (e) => {
    const p = pts.get(e.pointerId);
    if (!p) return;
    const dx = e.clientX - p.x, dy = e.clientY - p.y;
    p.x = e.clientX; p.y = e.clientY;
    moved += Math.abs(dx) + Math.abs(dy);
    const r = pad.getBoundingClientRect();
    pad.style.setProperty("--x", ((e.clientX - r.left) / r.width) * 100 + "%");
    pad.style.setProperty("--y", ((e.clientY - r.top) / r.height) * 100 + "%");
    if (pts.size >= 2) { acc.sy += -dy / 18; acc.sx += dx / 18; }
    else {
      const speed = Math.hypot(dx, dy);
      const gain = 1.4 + Math.min(3.2, speed * 0.12);
      acc.dx += dx * gain; acc.dy += dy * gain;
    }
    if (!raf) raf = requestAnimationFrame(flush);
  });
  const up = (e) => {
    if (!pts.has(e.pointerId)) return;
    pts.delete(e.pointerId);
    if (pts.size === 0) {
      pad.classList.remove("touch");
      if (Date.now() - startT < 240 && moved < 10 && app.link) { app.link.fire({ op: "click", b: maxFingers >= 2 ? "right" : "left" }); buzz(5); }
    }
  };
  pad.addEventListener("pointerup", up);
  pad.addEventListener("pointercancel", up);
})();

// ------------------------------------------------------------------ pairing (welcome, scanner)

function showWelcome() {
  $("welcome").hidden = false;
  $("main").hidden = true;
  $("tabs").hidden = true;
  $("notch").hidden = true;
}
$("wScan").addEventListener("click", scan);
$("wPaste").addEventListener("click", async () => {
  let t = "";
  try { t = await navigator.clipboard.readText(); } catch (_) {}
  if (!t) t = prompt("Paste the link from your computer") || "";
  const p = parsePair(t);
  if (!p) { toast("That isn't an OpenHop pairing link"); return; }
  addPc(p);
  location.replace(location.pathname);
});

let camStream = null, scanTimer = null;
async function scan() {
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) { toast("This browser can't use the camera here"); return; }
  if (!window.jsQR) await new Promise((res, rej) => { const s = el("script"); s.src = "vendor/jsQR.js"; s.onload = res; s.onerror = rej; document.head.append(s); });
  openSheet("scanSheet");
  try { camStream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } }); }
  catch (_) { closeSheets(); toast("Allow the camera to scan the code"); return; }
  const v = $("cam");
  v.srcObject = camStream;
  await v.play().catch(() => {});
  const c = el("canvas"), g = c.getContext("2d", { willReadFrequently: true });
  const tick = () => {
    if (!camStream) return;
    if (v.videoWidth) {
      const w = 480, h = Math.round((v.videoHeight / v.videoWidth) * w);
      c.width = w; c.height = h;
      g.drawImage(v, 0, 0, w, h);
      const code = window.jsQR(g.getImageData(0, 0, w, h).data, w, h, { inversionAttempts: "dontInvert" });
      const p = code && parsePair(code.data);
      if (p) { buzz(30); stopScan(); addPc(p); location.replace(location.pathname); return; }
    }
    scanTimer = setTimeout(tick, 120);
  };
  tick();
}
function stopScan() { clearTimeout(scanTimer); if (camStream) camStream.getTracks().forEach((t) => t.stop()); camStream = null; }

// ------------------------------------------------------------------ install

let installEvent = null;
window.addEventListener("beforeinstallprompt", (e) => { e.preventDefault(); installEvent = e; renderInstall(); });
function renderInstall() {
  const standalone = matchMedia("(display-mode: standalone)").matches || navigator.standalone;
  $("installBtn").hidden = !installEvent || standalone;
  $("installHelp").hidden = !(IOS && !standalone && !LAN);
  fillIcons($("installHelp"));
}
$("installBtn").addEventListener("click", async () => { if (!installEvent) return; installEvent.prompt(); await installEvent.userChoice.catch(() => {}); installEvent = null; renderInstall(); });
if ("serviceWorker" in navigator && location.protocol === "https:") navigator.serviceWorker.register("sw.js").catch(() => {});

start();
