import { createEffect, onCleanup, Show, type Component, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
// One icon by its own path: the package's index pulls in every icon,
// thousands of modules in development.
import X from "lucide-solid/icons/x";
import "./ui.css";
import { t } from "./i18n";
import { opened, takesEscape } from "./layers";

/**
 * Minimal modal dialog: fixed overlay, centered scrollable card, closes on
 * Escape, overlay click, or the header button. Rendered through a portal so
 * host stacking contexts never clip it. `locked` keeps it open (no close
 * button, Escape or overlay click) while something must not be interrupted.
 * Opened, it takes the focus (unless a field of its own did) and gives it
 * back when it closes; opened from another dialog, Escape closes only it.
 */
export const Modal: Component<{
  open: boolean;
  title: string;
  onClose: () => void;
  locked?: boolean;
  /** `wide`: almost the whole screen, for content that needs room (a flow's documentation). */
  size?: "wide";
  children: JSX.Element;
}> = (props) => {
  let box: HTMLDivElement | undefined;
  createEffect(() => {
    if (!props.open) return;
    const layer = opened();
    const before = document.activeElement;
    queueMicrotask(() => {
      if (box && !box.contains(document.activeElement)) box.focus();
    });
    const onKey = (e: KeyboardEvent) => {
      if (takesEscape(layer, e) && !props.locked) props.onClose();
    };
    document.addEventListener("keydown", onKey);
    onCleanup(() => {
      layer.closed();
      document.removeEventListener("keydown", onKey);
      if (before instanceof HTMLElement && before.isConnected) before.focus();
    });
  });

  return (
    <Show when={props.open}>
      <Portal>
        <div
          class="fk-modal__overlay"
          onClick={(e) => {
            if (e.target === e.currentTarget && !props.locked) props.onClose();
          }}
        >
          <div
            ref={box}
            tabIndex={-1}
            class="fk-modal"
            classList={{ "fk-modal--wide": props.size === "wide" }}
            role="dialog"
            aria-modal="true"
            aria-label={props.title}
          >
            <header class="fk-modal__header">
              <h2 class="fk-modal__title">{props.title}</h2>
              <Show when={!props.locked}>
                <button
                  type="button"
                  class="fk-btn fk-btn--sm"
                  aria-label={t("kit:close")}
                  onClick={() => props.onClose()}
                >
                  <X size={16} />
                </button>
              </Show>
            </header>
            <div class="fk-modal__body">{props.children}</div>
          </div>
        </div>
      </Portal>
    </Show>
  );
};
