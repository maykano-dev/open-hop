"use strict";
// Shared by every OpenHop window: light or dark (always following the
// computer's own setting) and the accent colour.
(function () {
  const ACCENTS = {
    blue: ["#007aff", "#0a84ff"], purple: ["#af52de", "#bf5af2"], pink: ["#ff2d55", "#ff375f"],
    orange: ["#ff9500", "#ff9f0a"], green: ["#34c759", "#30d158"], graphite: ["#8e8e93", "#98989d"],
  };
  const media = matchMedia("(prefers-color-scheme: dark)");
  let current = { accent: "blue", sys: null };
  // `systemDark`: what OpenHop read from the OS itself (some Linux web views
  // don't pass the desktop's dark mode on); null to go by the web view.
  function apply(_unused, accent, systemDark) {
    current = { accent: accent || current.accent || "blue", sys: typeof systemDark === "boolean" ? systemDark : current.sys };
    const root = document.documentElement;
    const dark = typeof current.sys === "boolean" ? current.sys : media.matches;
    root.dataset.theme = dark ? "dark" : "light";
    const a = ACCENTS[current.accent] || ACCENTS.blue;
    root.style.setProperty("--blue", a[dark ? 1 : 0]);
    root.style.setProperty("--accent", a[dark ? 1 : 0]);
  }
  media.addEventListener("change", () => { current.sys = null; apply(null, current.accent); });
  window.OpenHopTheme = { apply, ACCENTS };
})();
