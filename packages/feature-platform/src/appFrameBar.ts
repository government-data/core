/**
 * Quality Data's avatar over an application's top bar (AppFrame): centered
 * on it, whatever its height (RustFS's is 4rem, Grafana's lower), and with an
 * ink that reads over its background (Trino's bar is dark navy while Quality
 * Data is light). The page's CSS reads `--app-frame-bar` and `--app-frame-ink`.
 */

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Measure the application's top bar (`selector` in its page) and, if its
 * frame is shown, put the avatar over it. Its page draws it after loading:
 * it is looked for a few times.
 */
export async function measureBar(frame: HTMLIFrameElement, selector: string) {
  for (let i = 0; i < 30; i++) {
    const bar = frame.contentDocument?.querySelector<HTMLElement>(selector);
    const height = bar?.getBoundingClientRect().height;
    if (bar && height) {
      frame.dataset.bar = `${Math.round(height)}px`;
      frame.dataset.ink = inkOver(bar);
      if (!frame.hidden) showBar(frame);
      return;
    }
    await sleep(100);
  }
}

/** The avatar's bar as the application's (or Quality Data's own, without one). */
export function showBar(frame?: HTMLIFrameElement) {
  const style = document.body.style;
  const height = frame?.dataset.bar;
  const ink = frame?.dataset.ink;
  if (height) style.setProperty("--app-frame-bar", height);
  else style.removeProperty("--app-frame-bar");
  if (ink) style.setProperty("--app-frame-ink", ink);
  else style.removeProperty("--app-frame-ink");
}

/**
 * A light or dark ink for text over an element: from the first background
 * that is not transparent, going up from it.
 */
function inkOver(el: HTMLElement): string {
  const view = el.ownerDocument.defaultView;
  for (let node: HTMLElement | null = el; node && view; node = node.parentElement) {
    const rgba = toRgba(view.getComputedStyle(node).backgroundColor);
    if (!rgba || rgba[3] === 0) continue;
    const [r, g, b] = rgba;
    return 0.2126 * r + 0.7152 * g + 0.0722 * b < 128 ? "#f5f5f7" : "#1c1c22";
  }
  return "";
}

/**
 * Any CSS color as sRGB bytes, by painting it: the applications write theirs
 * as `rgb()`, `oklch()` (RustFS) or `color()`, and the browser converts them.
 */
function toRgba(color: string): number[] | undefined {
  const ctx = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  if (!ctx) return undefined;
  ctx.fillStyle = color;
  ctx.fillRect(0, 0, 1, 1);
  return [...ctx.getImageData(0, 0, 1, 1).data];
}
