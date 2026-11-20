import { For, type Component } from "solid-js";
import { t } from "./i18n";
import { options, type Filter, type Row } from "./rules";

/** One filter: a select of the values a column takes, or any. */
const Choose: Component<{
  label: string;
  values: string[];
  value: string;
  onChange: (value: string) => void;
}> = (props) => (
  <label class="qd-field">
    <span>{props.label}</span>
    <select value={props.value} onChange={(e) => props.onChange(e.currentTarget.value)}>
      <option value="">{t("quality:rules.any")}</option>
      <For each={props.values}>{(v) => <option value={v}>{v}</option>}</For>
    </select>
  </label>
);

/** The row of filters above a dataset's rules. */
export const RulesFilters: Component<{
  rows: Row[];
  filter: Filter;
  onChange: (filter: Filter) => void;
}> = (props) => {
  const set = (key: keyof Filter) => (value: string) =>
    props.onChange({ ...props.filter, [key]: value });
  return (
    <div class="qd-filters">
      <label class="qd-field">
        <span>{t("quality:rules.search")}</span>
        <input
          type="search"
          value={props.filter.text}
          placeholder={t("quality:rules.searchHint")}
          onInput={(e) => set("text")(e.currentTarget.value)}
        />
      </label>
      <Choose
        label={t("quality:rules.col.pattern")}
        values={options(props.rows, "pattern")}
        value={props.filter.pattern}
        onChange={set("pattern")}
      />
      <Choose
        label={t("quality:rules.col.dimension")}
        values={options(props.rows, "dimension")}
        value={props.filter.dimension}
        onChange={set("dimension")}
      />
      <Choose
        label={t("quality:rules.col.severity")}
        values={options(props.rows, "severity")}
        value={props.filter.severity}
        onChange={set("severity")}
      />
    </div>
  );
};
