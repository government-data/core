import { For, Show, type Component } from "solid-js";
import type { DatasetOut } from "./api";
import { t } from "./i18n";

/** The dataset whose cuts, schedules or rules the page shows. */
export const DatasetPicker: Component<{
  datasets: DatasetOut[];
  value: string;
  onChange: (name: string) => void;
}> = (props) => (
  <Show when={props.datasets.length > 0}>
    <label class="qd-field qd-inline">
      <span>{t("quality:cuts.dataset")}</span>
      <select value={props.value} onChange={(e) => props.onChange(e.currentTarget.value)}>
        <For each={props.datasets}>{(d) => <option value={d.name}>{d.name}</option>}</For>
      </select>
    </label>
  </Show>
);
