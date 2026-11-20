import { createSignal, For, Show, type Component } from "solid-js";
import { t } from "./i18n";

/**
 * Toast notifications every feature shares: the outcome of an action, an
 * error, a notice. One store for the app; the shell mounts the
 * {@link Toaster} once, outside the pages, so a toast stays in sight from
 * any section.
 */

export type ToastKind = "success" | "error" | "info" | "warning";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  body?: string;
}

const [toasts, setToasts] = createSignal<Toast[]>([]);
let nextId = 1;
const MAX_VISIBLE = 5;

export function dismiss(id: number) {
  setToasts((all) => all.filter((x) => x.id !== id));
}

/**
 * Shows a toast. `ms`: how long it stays (5 s by default; errors, 9 s);
 * `null` keeps it until the person closes it.
 */
export function toast(kind: ToastKind, title: string, body?: string, ms?: number | null) {
  // The same one in sight already: once is enough.
  if (toasts().some((x) => x.kind === kind && x.title === title && x.body === body)) return;
  const id = nextId++;
  setToasts((all) => [...all, { id, kind, title, body }].slice(-MAX_VISIBLE));
  if (ms !== null) setTimeout(() => dismiss(id), ms ?? (kind === "error" ? 9000 : 5000));
}

const ICON: Record<ToastKind, string> = {
  success: "✓",
  error: "✕",
  info: "i",
  warning: "!",
};

/** The toasts, bottom right, each with its close button. */
export const Toaster: Component = () => (
  <div
    class="fk-toaster"
    role="region"
    aria-label={t("kit:toast.notifications")}
    aria-live="polite"
  >
    <For each={toasts()}>
      {(x) => (
        <div class={`fk-toast fk-toast--${x.kind}`} role={x.kind === "error" ? "alert" : "status"}>
          <span class="fk-toast__icon" aria-hidden="true">
            {ICON[x.kind]}
          </span>
          <div class="fk-toast__text">
            <strong>{x.title}</strong>
            <Show when={x.body}>
              <span>{x.body}</span>
            </Show>
          </div>
          <button
            type="button"
            class="fk-toast__close"
            aria-label={t("kit:toast.close")}
            onClick={() => dismiss(x.id)}
          >
            ×
          </button>
        </div>
      )}
    </For>
  </div>
);
