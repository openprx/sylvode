try {
  var saved = localStorage.getItem("sv-site-theme");
  if (saved === "light" || saved === "dark") document.documentElement.dataset.theme = saved;
} catch (_) {}
