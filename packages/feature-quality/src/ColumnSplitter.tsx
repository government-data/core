import { createSignal, type Component } from "solid-js";

const KEY = "qd.editor.compare";
/** Neither column narrower than this share of the two. */
const MIN = 0.2;

/** A share both columns fit in; halves for anything that is no number. */
const clamp = (share: number) =>
  Number.isFinite(share) ? Math.max(MIN, Math.min(1 - MIN, share)) : 0.5;

/** The rule's column's share of the two, kept between visits (0.5 at first). */
export function createColumnShare() {
  let saved = Number.NaN;
  try {
    saved = Number(localStorage.getItem(KEY));
  } catch {
    // Not kept in this browser: halves.
  }
  const [share, setShare] = createSignal(saved > 0 ? clamp(saved) : 0.5);
  return {
    share,
    set: (s: number) => {
      setShare(clamp(s));
      try {
        localStorage.setItem(KEY, String(share()));
      } catch {
        // Kept for this page only.
      }
    },
  };
}

/**
 * The bar between the rule and the console (decision 0054): dragged, or
 * moved with the arrow keys, it gives one column more room and the other
 * less. `box` is the element both columns share.
 */
export const ColumnSplitter: Component<{
  share: number;
  box: () => HTMLElement | undefined;
  label: string;
  onResize: (share: number) => void;
}> = (props) => {
  const drag = (down: PointerEvent) => {
    const bar = down.currentTarget as HTMLElement;
    const box = props.box()?.getBoundingClientRect();
    if (!box || box.width <= 0) return;
    bar.setPointerCapture(down.pointerId);
    const move = (e: PointerEvent) => props.onResize((e.clientX - box.left) / box.width);
    const up = () => {
      bar.removeEventListener("pointermove", move);
      bar.removeEventListener("pointerup", up);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up);
    down.preventDefault();
  };
  const key = (e: KeyboardEvent) => {
    const step = e.key === "ArrowLeft" ? -0.02 : e.key === "ArrowRight" ? 0.02 : 0;
    if (!step) return;
    props.onResize(props.share + step);
    e.preventDefault();
  };
  return (
    <div
      class="qd-column-splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      aria-valuenow={Math.round(props.share * 100)}
      tabindex="0"
      onPointerDown={drag}
      onKeyDown={key}
    />
  );
};
