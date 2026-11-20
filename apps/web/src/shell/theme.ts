import { createSignal } from "solid-js";

/**
 * The theme: light, dark, or the system's (`auto`). Kept where the
 * documentation keeps it (Starlight's `starlight-theme`: empty, the system's),
 * so the app and the docs share it; applied as `data-theme` on <html>, which
 * theme.css honours over the system's preference.
 */
export type Theme = "light" | "dark" | "auto";

export const THEMES: Theme[] = ["light", "dark", "auto"];

const STORAGE_KEY = "starlight-theme";

function stored(): Theme {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return v === "light" || v === "dark" ? v : "auto";
  } catch {
    return "auto";
  }
}

const [current, setCurrent] = createSignal<Theme>(stored());

/** The theme chosen. */
export const theme = current;

function apply(t: Theme) {
  if (t === "auto") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = t;
}

/** Choose the theme: applied at once and remembered in this browser. */
export function setTheme(t: Theme): void {
  try {
    localStorage.setItem(STORAGE_KEY, t === "auto" ? "" : t);
  } catch {
    // Without storage, it lasts until a reload.
  }
  apply(t);
  setCurrent(t);
}

/** Apply the remembered theme, before the app paints. */
export function initTheme(): void {
  apply(current());
}
