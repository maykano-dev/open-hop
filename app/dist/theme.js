"use strict";
// Shared by every OpenHop window: light/dark/system and the accent colour.
(function () {
  const ACCENTS = {
    blue: ["#007aff", "#0a84ff"], purple: ["#af52de", "#bf5af2"], pink: ["#ff2d55", "#ff375f"],
    orange: ["#ff9500", "#ff9f0a"], green: ["#34c759", "#30d158"], graphite: ["#8e8e93", "#98989d"],
  };
  let current = { appearance: "system", accent: "blue" };
  function apply(appearance, accent) {
    current = { appearance: appearance || "system", accent: accent || "blue" };
    const root = document.documentElement;
    if (current.appearance === "light" || current.appearance === "dark") root.dataset.theme = current.appearance;
    else delete root.dataset.theme;
    const dark = current.appearance === "dark" || (current.appearance !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
    const a = ACCENTS[current.accent] || ACCENTS.blue;
    root.style.setProperty("--blue", a[dark ? 1 : 0]);
    root.style.setProperty("--accent", a[dark ? 1 : 0]);
  }
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => apply(current.appearance, current.accent));
  window.OpenHopTheme = { apply, ACCENTS };
})();
