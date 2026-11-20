import { createEffect, createResource, For, Show, type Component } from "solid-js";
import { A } from "@solidjs/router";
import { lakeApi, sourcesApi, type DatasetOut, type ListingView } from "./api";
import { AddRule } from "./AddRule";
import { BranchMark } from "./ColumnHistory";
import { bronzeOf, queryMenu } from "./Console";
import { deleteMenu } from "./DeleteDialog";
import { Branch } from "./Branch";
import { DatasetPlan } from "./DatasetPlan";
import { columnsOf, datasetTable, matches, readsWhat, type TableNode } from "./explorer";
import { datasetPlace, pick, picked, revealed, ruled, rulesOf } from "./explorerCache";
import { MapTable } from "./MapTable";
import { GLOBAL_FILE } from "./newRule";
import { SplitByField } from "./SplitByField";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import { ColumnLeaf, DerivedFields, Info, MarkOf, RuleItem } from "./TreeLeaves";
import { openMenu } from "./TreeMenu";

/** Where a dataset's node is in the tree, and the element it is scrolled to by. */
const anchorOf = (dataset: string) => `qd-dataset-${dataset}`;

/** A dataset's fields against its table's columns, the rules under each, and what it plans. */
const DatasetColumns: Component<{
  source?: string;
  table: TableNode;
  dataset: DatasetOut;
  pipelines: boolean;
}> = (props) => {
  const [rules] = createResource(
    () => ({ dataset: props.dataset.name, n: ruled() }),
    (k) => rulesOf(k.dataset),
  );
  const tree = () => columnsOf(props.table, props.dataset, rules());
  const place = () => datasetPlace(props.dataset.name);
  return (
    <>
      <LoadFailed when={rules.error} />
      <li role="treeitem" class="qd-tree-leaf">
        <A
          class="qd-link"
          href={`/quality/rules/${encodeURIComponent(props.dataset.name)}/${GLOBAL_FILE}`}
        >
          ◆ {t("quality:explorer.globalRules")}
        </A>
        <SplitByField dataset={props.dataset.name} />
        <MapTable
          dataset={props.dataset.name}
          columns={tree().columns.filter((c) => !c.fields.length && c.mark !== "gone")}
        />
      </li>
      <For each={tree().columns}>
        {(column) => (
          <Show
            when={column.fields.length}
            fallback={
              <ColumnLeaf
                name={column.name}
                dataType={column.dataType}
                source={props.source}
                table={props.table}
              />
            }
          >
            <Branch
              id={`${place()}/${column.name}`}
              class={`qd-tree-column qd-tree--${column.mark}`}
              label={
                <span>
                  <span class="qd-mono">{column.name}</span>{" "}
                  <Info source={props.source} table={props.table} column={column.name} />{" "}
                  <small class="qd-hint">{column.dataType}</small> <MarkOf mark={column.mark} />
                  <Show when={column.gap}>
                    <small class="qd-mark qd-mark--gap">{t("quality:explorer.mark.gap")}</small>
                  </Show>{" "}
                  <AddRule dataset={props.dataset.name} field={column.fields[0]} />
                </span>
              }
            >
              {() => (
                <For
                  each={column.rules}
                  fallback={<li class="qd-hint">{t("quality:explorer.noRules")}</li>}
                >
                  {(rule) => <RuleItem dataset={props.dataset.name} rule={rule} />}
                </For>
              )}
            </Branch>
          </Show>
        )}
      </For>
      <Show when={tree().derived.length}>
        <DerivedFields
          id={`${place()}/+derived`}
          dataset={props.dataset.name}
          fields={tree().derived}
        />
      </Show>
      <DatasetPlan dataset={props.dataset.name} pipelines={props.pipelines} />
    </>
  );
};

/** The listing a dataset's table is in: its source's, or the lake's; none if not to be seen. */
const listingOf = (dataset: DatasetOut): Promise<ListingView | undefined> =>
  (dataset.from ? sourcesApi.tables(dataset.from.source) : lakeApi.tables()).catch(() => undefined);

/**
 * A dataset as a node of the lake (decision 0049): named by itself, saying
 * what it reads, marked _no cuts yet_ before its first run, and under it
 * its fields with their rules, its global rules, schedules, cuts and
 * drafts. Marked when opened by a click on its row, or when a source's
 * table links to it: then scrolled to and left closed, as it was.
 */
export const DatasetBranch: Component<{ dataset: DatasetOut; pipelines: boolean }> = (props) => {
  const asked = () => revealed()?.name === props.dataset.name;
  createEffect(() => {
    if (!asked()) return;
    requestAnimationFrame(() =>
      document
        .getElementById(anchorOf(props.dataset.name))
        ?.scrollIntoView({ block: "nearest", behavior: "smooth" }),
    );
  });
  return (
    <Branch
      id={datasetPlace(props.dataset.name)}
      class="qd-tree-table qd-tree-dataset"
      selected={picked() === datasetPlace(props.dataset.name)}
      onPick={(opened) => pick(datasetPlace(props.dataset.name), opened)}
      onMenu={(e) =>
        openMenu(e, [
          ...queryMenu(bronzeOf(props.dataset)),
          ...deleteMenu({ dataset: props.dataset.name }),
        ])
      }
      label={
        <span id={anchorOf(props.dataset.name)}>
          <BranchMark dataset={props.dataset.name} label={props.dataset.name} />
          <A
            class="qd-tree-branch"
            href={`/quality/rules?${new URLSearchParams({ dataset: props.dataset.name }).toString()}`}
            title={t("quality:explorer.datasetRules", { dataset: props.dataset.name })}
            aria-label={t("quality:explorer.datasetRules", { dataset: props.dataset.name })}
            onClick={(e) => e.stopPropagation()}
          >
            ▤
          </A>{" "}
          <span class="qd-mono">◆ {props.dataset.name}</span>{" "}
          <small class="qd-hint">
            {t("quality:explorer.reads", { what: readsWhat(props.dataset) })}
          </small>{" "}
          <Show when={!props.dataset.last_cut}>
            <small class="qd-mark qd-mark--unread">{t("quality:explorer.noCuts")}</small>
          </Show>
        </span>
      }
    >
      {() => {
        const [listing] = createResource(() => props.dataset, listingOf);
        const table = () => datasetTable(props.dataset, listing());
        return (
          <Show when={!listing.loading}>
            <Show when={table().mark === "gone"}>
              <li class="qd-tree-note">
                <MarkOf mark="gone" />
              </li>
            </Show>
            <DatasetColumns
              source={props.dataset.from?.source}
              table={table()}
              dataset={props.dataset}
              pipelines={props.pipelines}
            />
          </Show>
        );
      }}
    </Branch>
  );
};

/** Every dataset, a node of the lake each, those the filter leaves out hidden. */
export const LakeDatasets: Component<{
  datasets: DatasetOut[];
  query: string;
  pipelines: boolean;
}> = (props) => (
  <For each={[...props.datasets].sort((a, b) => a.name.localeCompare(b.name))}>
    {(dataset) => (
      <Show
        when={
          !props.query.trim() ||
          matches(
            props.query,
            dataset.name,
            readsWhat(dataset),
            ...dataset.fields.map((f) => f.field),
          )
        }
      >
        <DatasetBranch dataset={dataset} pipelines={props.pipelines} />
      </Show>
    )}
  </For>
);
