import { For, type Component } from "solid-js";

/** A row of tabs; the page shows the panel of the one chosen. */
export const Tabs: Component<{
  tabs: { id: string; label: string }[];
  current: string;
  onChoose: (id: string) => void;
}> = (props) => (
  <div class="qd-tabs" role="tablist">
    <For each={props.tabs}>
      {(tab) => (
        <button
          type="button"
          role="tab"
          class="qd-tab"
          aria-selected={props.current === tab.id}
          onClick={() => props.onChoose(tab.id)}
        >
          {tab.label}
        </button>
      )}
    </For>
  </div>
);
