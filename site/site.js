// Download buttons: the file for each system from the latest release, and
// this visitor's system first.
(function () {
  var ua = navigator.userAgent, os = /Windows/.test(ua) ? "windows" : /Mac/.test(ua) && !/iPhone|iPad/.test(ua) ? "mac" : /Linux|X11/.test(ua) && !/Android/.test(ua) ? "linux" : "";
  var names = { windows: "Windows", mac: "macOS", linux: "Linux" };
  var card = os && document.querySelector('.dl[data-os="' + os + '"]');
  if (card) {
    card.classList.add("mine");
    card.parentNode.insertBefore(card, card.parentNode.firstChild);
    document.getElementById("heroDownload").textContent = "Download for " + names[os];
  }
  var pick = { windows: /x64-setup\.exe$/, mac: /\.dmg$/, linux: /\.AppImage$/ };
  fetch("https://api.github.com/repos/maykano-dev/open-hop/releases/latest")
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (rel) {
      if (!rel || !rel.assets) return;
      Object.keys(pick).forEach(function (k) {
        var a = rel.assets.find(function (x) { return pick[k].test(x.name); });
        var el = document.querySelector('.dl[data-os="' + k + '"]');
        if (a && el) {
          el.href = a.browser_download_url;
          el.querySelector("em").textContent = "Download " + rel.tag_name;
        }
      });
      if (card) document.getElementById("heroDownload").href = card.href;
    })
    .catch(function () {});
})();
