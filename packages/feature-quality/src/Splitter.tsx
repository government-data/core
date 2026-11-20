import { createSignal, type Component } from "solid-js";

const KEY = "qd.editor.panel";
const MIN = 72;

/** The panel's height, kept between visits. */
export function createPanelHeight(initial = 208) {
  const saved = Number(localStorage.getItem(KEY));
  const [height, setHeight] = createSignal(saved >= MIN ? saved : initial);
  return {
    height,
    set: (px: number) => {
      setHeight(px);
      localStorage.setItem(KEY, String(Math.round(px)));
    },
  };
}

/**
 * The bar between the editor and its panel: dragged, or moved with the
 * arrow keys, it gives one more room and the other less. `max` is the most
 * the panel may take.
 */
export const Splitter: Component<{
  height: number;
  max: () => number;
  label: string;
  onResize: (px: number) => void;
}> = (props) => {
  const clamp = (px: number) => Math.max(MIN, Math.min(props.max(), px));
  const drag = (down: PointerEvent) => {
    const bar = down.currentTarget as HTMLElement;
    const start = down.clientY;
    const from = props.height;
    bar.setPointerCapture(down.pointerId);
    const move = (e: PointerEvent) => props.onResize(clamp(from + start - e.clientY));
    const up = () => {
      bar.removeEventListener("pointermove", move);
      bar.removeEventListener("pointerup", up);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up);
    down.preventDefault();
  };
  const key = (e: KeyboardEvent) => {
    const step = e.key === "ArrowUp" ? 16 : e.key === "ArrowDown" ? -16 : 0;
    if (!step) return;
    props.onResize(clamp(props.height + step));
    e.preventDefault();
  };
  return (
    <div
      class="qd-splitter"
      role="separator"
      aria-orientation="horizontal"
      aria-label={props.label}
      aria-valuenow={Math.round(props.height)}
      tabindex="0"
      onPointerDown={drag}
      onKeyDown={key}
    />
  );
};
