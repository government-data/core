import { createSignal } from "solid-js";

/** Where the sidebar's width is remembered (per browser), while it holds a panel. */
const WIDTH_KEY = "orq.nav.width";
const MIN = 240;
const MAX = 720;

/** A width the sidebar may take: at least {@link MIN}, at most {@link MAX} and 70% of the window. */
export function clampWidth(px: number, viewport: number): number {
  return Math.round(Math.max(MIN, Math.min(px, MAX, viewport * 0.7)));
}

function readWidth(): number | undefined {
  try {
    const saved = Number(localStorage.getItem(WIDTH_KEY));
    return saved > 0 ? saved : undefined;
  } catch {
    return undefined;
  }
}

function saveWidth(px: number | undefined) {
  try {
    if (px === undefined) localStorage.removeItem(WIDTH_KEY);
    else localStorage.setItem(WIDTH_KEY, String(px));
  } catch {
    // Without storage the width just lasts until the next reload.
  }
}

/**
 * The sidebar's width while it holds a panel, chosen by dragging its edge
 * (or with the arrow keys on it), remembered per browser; a double click
 * goes back to the stylesheet's.
 */
export function useSidebarWidth() {
  const [width, setWidth] = createSignal(readWidth());
  const set = (px: number | undefined) => {
    const next = px === undefined ? undefined : clampWidth(px, window.innerWidth);
    setWidth(next);
    saveWidth(next);
  };
  return { width, set };
}
