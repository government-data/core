import { Show, For, type Component } from "solid-js";
import { A } from "@solidjs/router";
import type { DatasetOut } from "./api";
import { bronzeOf, queryMenu } from "./Console";
import { deleteMenu, type Deleting } from "./DeleteDialog";
import { MapTable } from "./MapTable";
import { openMenu } from "./TreeMenu";
import { loading, tableKey, type Loading } from "./mapWatch";
import { Branch } from "./Branch";
import { pick, picked, reveal } from "./explorerCache";
import { newDataset, readColumns, type TableNode } from "./explorer";
import { t } from "./i18n";
import { ColumnLeaf, Info, MarkOf } from "./TreeLeaves";

/** Where a table is in the tree: its source (none for the lake), schema and name. */
const tablePlace = (source: string | undefined, table: TableNode) =>
  `${source ?? ""}/${table.schema}.${table.name}`;

/** A link to the dataset that reads a table: its node in the lake, opened (decision 0049). */
const DatasetLink: Component<{ name: string }> = (props) => (
  <button
    type="button"
    class="qd-link qd-tree-goto"
    title={t("quality:explorer.openDataset", { name: props.name })}
    onClick={(e) => {
      e.preventDefault();
      reveal(props.name);
    }}
  >
    → {props.name}
  </button>
);

/**
 * A circle that turns beside a table being mapped and collected, until it
 * is ready; on hover, the step it is in.
 */
const LoadingMark: Component<{ source?: string; table: TableNode }> = (props) => {
  const now = () =>
    props.source === undefined
      ? undefined
      : loading().get(tableKey(props.source, `${props.table.schema}.${props.table.name}`));
  // What is being done, and since when: said on hover.
  const label = (l: Loading) =>
    t(`quality:explorer.loading.${l.at}`, {
      dataset: l.dataset,
      since: l.since.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
    });
  return (
    <Show when={now()}>
      {(l) => <span class="qd-spinner" role="status" aria-label={label(l())} title={label(l())} />}
    </Show>
  );
};

/**
 * A table of the tree, browsed (decision 0049): its marks, a link to each
 * dataset that reads it and, opened, its columns with their types; a table
 * no mapping reads offers to map it. Rules are under the dataset, never
 * here.
 */
export const ExplorerTable: Component<{
  /** None for the lake. */
  source?: string;
  table: TableNode;
  datasets: DatasetOut[];
  hidden?: boolean;
  onMap?: (table: string) => void;
}> = (props) => {
  // A table no dataset reads, but one of that name exists: nothing to start.
  const starts = () => {
    const start = newDataset(props.source, props.table);
    return start && !props.datasets.some((d) => d.name === start.dataset) ? start : undefined;
  };
  const read = () => readColumns(props.table, props.datasets);
  const place = () => tablePlace(props.source, props.table);
  // Where its records are in the lake: itself, or the bronze table of a
  // dataset that reads it.
  const inLake = (): [string, string] | undefined => {
    if (!props.source) return [props.table.schema, props.table.name];
    const d = props.datasets.find((x) => props.table.datasets.includes(x.name));
    return d && bronzeOf(d);
  };
  // A source's table: its dataset; a table of the lake: itself.
  const deletes = (): Deleting | undefined => {
    if (!props.source) return { namespace: props.table.schema, table: props.table.name };
    const d = props.table.datasets[0];
    return d ? { dataset: d } : undefined;
  };
  return (
    <Branch
      onMenu={(e) => openMenu(e, [...queryMenu(inLake()), ...deleteMenu(deletes())])}
      id={place()}
      hidden={props.hidden}
      selected={picked() === place()}
      onPick={(opened) => pick(place(), opened)}
      class={`qd-tree-table qd-tree--${props.table.mark}`}
      label={
        <span>
          <span class="qd-mono">{props.table.name}</span>{" "}
          <LoadingMark source={props.source} table={props.table} />
          <Info source={props.source} table={props.table} />{" "}
          <For each={props.table.datasets}>{(name) => <DatasetLink name={name} />}</For>{" "}
          <Show when={props.source && props.table.mark !== "gone"}>
            <A
              class="qd-link qd-tree-goto"
              href={`/quality/records?${new URLSearchParams({
                source: props.source ?? "",
                schema: props.table.schema,
                table: props.table.name,
              }).toString()}`}
            >
              {t("quality:records.open")}
            </A>{" "}
          </Show>
          <MarkOf mark={props.table.mark} />
        </span>
      }
    >
      {() => (
        <>
          <Show when={!props.onMap && starts()}>
            {(start) => (
              <li class="qd-tree-note">
                <MapTable
                  dataset={start().dataset}
                  columns={props.table.columns.map((c) => ({
                    name: c.name,
                    dataType: c.data_type,
                  }))}
                  start={start().table}
                />
              </li>
            )}
          </Show>
          <Show when={props.onMap && props.table.mark === "unread"}>
            <li class="qd-tree-note">
              <button
                type="button"
                class="fk-btn fk-btn--sm"
                onClick={() => props.onMap?.(`${props.table.schema}.${props.table.name}`)}
              >
                {t("quality:explorer.map")}
              </button>
            </li>
          </Show>
          <For each={props.table.columns}>
            {(column) => (
              <ColumnLeaf
                name={column.name}
                dataType={column.data_type}
                source={props.source}
                table={props.table}
                read={read().has(column.name.toLowerCase())}
              />
            )}
          </For>
        </>
      )}
    </Branch>
  );
};
