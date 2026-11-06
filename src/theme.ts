import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const STORAGE_KEY = "tap-theme";

export type ThemeChoice = "system" | "light" | "dark";

export function normalizeTheme(value: string | null | undefined): ThemeChoice {
  return value === "light" || value === "dark" ? value : "system";
}

export function applyTheme(value: string | null | undefined) {
  const theme = normalizeTheme(value);
  const root = document.documentElement;
  if (theme === "system") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = theme;
  }
  try {
    if (theme === "system") localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // Storage can be blocked. The document attribute still applies.
  }
}

export function bootTheme() {
  void invoke<{ theme?: string }>("get_settings")
    .then((settings) => applyTheme(settings.theme))
    .catch(() => {});
  void listen<{ theme?: string }>("settings-changed", (event) => {
    applyTheme(event.payload?.theme);
  }).catch(() => {});
}
