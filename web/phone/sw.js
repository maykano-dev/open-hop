// Keeps the app working without a connection to the web server. The newest
// version is used whenever the server answers quickly (so fixes arrive on the
// next launch, not the one after); otherwise the copy kept here.
const CACHE = "openhop-v2";
const SHELL = ["./", "index.html", "app.js", "app.css", "manifest.webmanifest", "icon-192.png", "apple-touch-icon.png", "vendor/secp256k1.js"];
self.addEventListener("install", (e) => { e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting())); });
self.addEventListener("activate", (e) => {
  e.waitUntil(caches.keys().then((ks) => Promise.all(ks.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim()));
});
self.addEventListener("fetch", (e) => {
  const u = new URL(e.request.url);
  if (e.request.method !== "GET" || u.origin !== location.origin) return;
  e.respondWith(caches.open(CACHE).then(async (c) => {
    const hit = await c.match(e.request, { ignoreSearch: true });
    const fresh = fetch(e.request, { cache: "no-cache" }).then((r) => { if (r.ok) c.put(e.request, r.clone()); return r; });
    if (!hit) return fresh;
    // Network first, but never wait long for it.
    return Promise.race([fresh.catch(() => hit), new Promise((res) => setTimeout(() => res(hit), 2500))]);
  }));
});
