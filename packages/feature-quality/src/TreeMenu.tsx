import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { Portal } from "solid-js/web";

/** One choice of a row's menu. */
export interface MenuItem {
  label: string;
  run: () => void;
}

/** The menu open: where the click was, and its choices. */
const [menu, setMenu] = createSignal<{ x: number; y: number; items: MenuItem[] }>();

/** A row's menu, opened by a right click on it; nothing when it has no choice. */
export function openMenu(e: MouseEvent, items: MenuItem[]) {
  if (items.length === 0) return;
  e.preventDefault();
  e.stopPropagation();
  setMenu({ x: e.clientX, y: e.clientY, items });
}

/**
 * The tree's menu, where it was asked; closed by a choice, a click
 * elsewhere, Escape or a scroll.
 */
export const TreeMenuHost: Component = () => {
  const close = () => setMenu(undefined);
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
  onMount(() => {
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", close, true);
  });
  onCleanup(() => {
    window.removeEventListener("pointerdown", close);
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("scroll", close, true);
  });
  return (
    <Show when={menu()}>
      {(m) => (
        // Out of the side panel, whose colours are its own: the page's.
        <Portal>
          <ul
            class="qd-tree-menu"
            role="menu"
            style={{ left: `${m().x}px`, top: `${m().y}px` }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            <For each={m().items}>
              {(item) => (
                <li role="none">
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      close();
                      item.run();
                    }}
                  >
                    {item.label}
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Portal>
      )}
    </Show>
  );
};
