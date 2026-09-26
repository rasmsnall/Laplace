// picks light or dark before the first paint, so the page does not flash
(() => {
  let saved = null;
  try { saved = localStorage.getItem("theme"); } catch {}
  const system = matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  document.documentElement.dataset.theme = saved ?? system;
})();
