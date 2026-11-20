import { For, type Component } from "solid-js";
import "./ui.css";

const DOTS = [0, 1, 2, 3, 4, 5, 6, 7];

/**
 * Waiting: eight dots in a circle that grow and fade in a chain, in the accent
 * color. CSS only, without libraries or inline styles (the CSP does not allow
 * them). It is decorative: whoever uses it puts the text of what is awaited
 * beside it (with `role="status"`).
 */
export const Spiral: Component<{ class?: string }> = (props) => (
  <span class={`fk-spiral ${props.class ?? ""}`} aria-hidden="true">
    <For each={DOTS}>{() => <i />}</For>
  </span>
);

/** Over the page being left, until the other application paints. */
export const Leaving: Component<{ message: string }> = (props) => (
  <div class="fk-leaving">
    <Spiral />
    <p role="status">{props.message}</p>
  </div>
);
