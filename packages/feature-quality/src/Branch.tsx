import { createEffect, createSignal, Show, type Component, type JSX } from "solid-js";

/** The branches open, by `id`, while the page is open. */
const OPEN = new Set<string>();

/**
 * A level of the tree: its children are made, and asked for, the first time
 * it is opened, and kept while it is closed or hidden. A filter opens it;
 * clearing the filter leaves it as it is. With an `id`, a branch made again
 * (its data asked for again) opens as it was.
 */
export const Branch: Component<{
  label: JSX.Element;
  class?: string;
  open?: boolean;
  /** Where it is in the tree, to remember whether it was open. */
  id?: string;
  /** Left out by a filter, its state kept. */
  hidden?: boolean;
  /** The one picked: its row and what it holds are marked. */
  selected?: boolean;
  /** A right click on its row: its menu. */
  onMenu?: (e: MouseEvent) => void;
  /** Said when it is opened (`true`) or closed (`false`) by a click on it. */
  onPick?: (opened: boolean) => void;
  children: () => JSX.Element;
}> = (props) => {
  const was = props.id !== undefined && OPEN.has(props.id);
  const [open, setOpen] = createSignal(was);
  const [seen, setSeen] = createSignal(was);
  createEffect(() => props.open && setOpen(true));
  createEffect(() => open() && setSeen(true));
  createEffect(() => {
    if (props.id === undefined) return;
    if (open()) OPEN.add(props.id);
    else OPEN.delete(props.id);
  });
  return (
    <li role="treeitem" aria-expanded={open()} hidden={props.hidden}>
      <details
        class={`${props.class ?? ""}${props.selected ? " qd-tree--selected" : ""}`}
        open={open()}
        onToggle={(e) => setOpen(e.currentTarget.open)}
      >
        <summary
          onContextMenu={(e) => props.onMenu?.(e)}
          onClick={(e) => {
            // A link or a button in its row does its own, and neither opens nor picks it.
            if (!(e.target as Element).closest("a, button")) props.onPick?.(!open());
          }}
        >
          {props.label}
        </summary>
        <Show when={seen()}>
          <ul role="group">{props.children()}</ul>
        </Show>
      </details>
    </li>
  );
};
