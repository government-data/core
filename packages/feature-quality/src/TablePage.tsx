import "./quality.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { lakeApi, rulesApi, sourcesApi, type DatasetOut } from "./api";
import { DatasetCharts } from "./DatasetCharts";
import { columnsOf, findTable, lakeTables, sourceSchemas, type TableNode } from "./explorer";
import { ruled, rulesOf } from "./explorerCache";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import { Tabs } from "./Tabs";

const same = (a: string | undefined, b: string) => a?.toLowerCase() === b.toLowerCase();

/** A table's columns as one dataset reads them: type, field, mark and rules, the one asked marked. */
const DatasetColumnsTable: Component<{
  table: TableNode;
  dataset: DatasetOut;
  column?: string;
}> = (props) => {
  const [rules] = createResource(
    () => ({ dataset: props.dataset.name, n: ruled() }),
    (k) => rulesOf(k.dataset),
  );
  const columns = () => columnsOf(props.table, props.dataset, rules()).columns;
  const open = (file: string, id: string) =>
    `/quality/rules/${encodeURIComponent(props.dataset.name)}/${encodeURIComponent(file)}?rule=${encodeURIComponent(id)}`;
  return (
    <div class="qd-scroll">
      <table class="table qd-table">
        <thead>
          <tr>
            <th>{t("quality:explorer.props.column")}</th>
            <th>{t("quality:explorer.props.type")}</th>
            <th>{t("quality:explorer.props.field")}</th>
            <th>{t("quality:explorer.props.rules")}</th>
          </tr>
        </thead>
        <tbody>
          <For each={columns()}>
            {(c) => (
              <tr
                classList={{
                  "qd-row--chosen": same(props.column, c.name),
                  [`qd-tree--${c.mark}`]: true,
                }}
              >
                <td class="qd-mono">
                  {c.name}
                  <Show when={c.mark !== "read"}>
                    <small class={`qd-mark qd-mark--${c.mark}`}>
                      {t(`quality:explorer.mark.${c.mark}`)}
                    </small>
                  </Show>
                </td>
                <td class="qd-mono qd-hint">{c.dataType ?? "—"}</td>
                <td class="qd-mono">{c.fields.join(", ") || "—"}</td>
                <td>
                  <Show when={c.gap}>
                    <small class="qd-mark qd-mark--gap">{t("quality:explorer.mark.gap")}</small>
                  </Show>
                  <For each={c.rules}>
                    {(r, i) => (
                      <>
                        {i() > 0 && ", "}
                        <A class="qd-link qd-mono" href={open(r.file, r.id)}>
                          {r.id}
                        </A>
                      </>
                    )}
                  </For>
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
};

/** The table asked, from its source's last listing (or the lake's), and the datasets. */
async function load(at: { source?: string; schema: string; table: string }) {
  const [listing, datasets] = await Promise.all([
    at.source ? sourcesApi.tables(at.source) : lakeApi.tables(),
    rulesApi.datasets(),
  ]);
  const schemas = at.source
    ? sourceSchemas(at.source, listing, datasets)
    : lakeTables(listing, datasets);
  return { table: findTable(schemas, at.schema, at.table), datasets };
}

/**
 * A table's properties (decision 0043), chosen in the explorer without a
 * file: where it is, the datasets that read it, and its columns with their
 * types, fields, marks and the rules that read them; a column asked for
 * (`?column=`) marked.
 */
export const TablePage: Component = () => {
  const [search] = useSearchParams<{
    source?: string;
    schema?: string;
    table?: string;
    column?: string;
  }>();
  const [found] = createResource(
    () => ({ source: search.source, schema: search.schema ?? "", table: search.table ?? "" }),
    load,
  );
  const [view, setView] = createSignal<"columns" | "charts">("columns");
  const reading = () => {
    const f = found();
    return f?.table ? f.datasets.filter((d) => f.table!.datasets.includes(d.name)) : [];
  };
  return (
    <section class="feature qd-page">
      <header class="qd-head">
        <div>
          <h1 class="qd-mono">
            {search.schema}.{search.table}
          </h1>
          <p class="qd-hint">
            {search.source
              ? t("quality:explorer.props.inSource", { source: search.source })
              : t("quality:explorer.props.inLake")}
          </p>
        </div>
      </header>
      <LoadFailed when={found.error} />
      <Show when={found() && !found()!.table}>
        <p class="qd-hint">{t("quality:explorer.props.notListed")}</p>
      </Show>
      <Show when={found()?.table}>
        {(table) => (
          <div class="card">
            <Show
              when={reading().length}
              fallback={<p class="qd-hint">{t("quality:explorer.props.noDataset")}</p>}
            >
              <Tabs
                tabs={[
                  { id: "columns", label: t("quality:charts.tabColumns") },
                  { id: "charts", label: t("quality:charts.tabCharts") },
                ]}
                current={view()}
                onChoose={(id) => setView(id as "columns" | "charts")}
              />
              <For each={reading()}>
                {(dataset) => (
                  <>
                    <h2>{t("quality:explorer.dataset", { name: dataset.name })}</h2>
                    <Show
                      when={view() === "charts"}
                      fallback={
                        <DatasetColumnsTable
                          table={table()}
                          dataset={dataset}
                          column={search.column}
                        />
                      }
                    >
                      <DatasetCharts dataset={dataset.name} />
                    </Show>
                  </>
                )}
              </For>
            </Show>
            <Show when={!reading().length}>
              <ul class="qd-table-list">
                <For each={table().columns}>
                  {(c) => (
                    <li classList={{ "qd-row--chosen": same(search.column, c.name) }}>
                      <span class="qd-mono">{c.name}</span>{" "}
                      <small class="qd-hint">{c.data_type}</small>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </div>
        )}
      </Show>
    </section>
  );
};
