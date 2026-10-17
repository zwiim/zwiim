// Paints the saved theme before the bundle loads so a dark window never flashes white.
// Mirrors src/lib/theme.ts; an external file because the CSP forbids inline scripts.
try {
  var pref = JSON.parse(localStorage.getItem("zwiim.desktop.theme") || '"system"');
  var dark = pref === "dark" || (pref !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
  if (dark) document.documentElement.classList.add("dark");
} catch (_) {}
