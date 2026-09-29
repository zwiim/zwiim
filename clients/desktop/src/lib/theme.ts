import { theme, type Theme } from "@/lib/prefs";

const systemDark = () => window.matchMedia?.("(prefers-color-scheme: dark)");

/** Apply the chosen theme (the remembered one by default) to the window. */
export function applyTheme(value: Theme = theme()): void {
  const dark = value === "dark" || (value === "system" && Boolean(systemDark()?.matches));
  document.documentElement.classList.toggle("dark", dark);
}

/** Apply the remembered theme now, and again whenever the system's changes. */
export function watchTheme(): void {
  applyTheme();
  systemDark()?.addEventListener("change", () => applyTheme());
}
