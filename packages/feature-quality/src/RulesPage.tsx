import "./quality.css";
import {
  createEffect,
  createResource,
  createSignal,
  For,
  on,
  Show,
  type Component,
} from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { Status } from "@knime-orchestador/feature-kit/ui";
import { rulesApi } from "./api";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import {
  broken,
  BY_ID,
  keep,
  NO_FILTER,
  rows,
  sorted,
  type Filter,
  type Row,
  type Sort,
  type SortKey,
} from "./rules";
import { RulesFilters } from "./RulesFilters";
import { RulesIO } from "./RulesIO";
import { ruled } from "./explorerCache";

const COLUMNS: SortKey[] = ["id", "field", "pattern", "dimension", "severity", "fails", "file"];

const ariaSort = (sort: Sort, key: SortKey) =>
  sort.key !== key ? "none" : sort.descending ? "descending" : "ascending";

/** A dataset's rules as a table, one row each, sorted by the header chosen. */
const RulesTable: Component<{
  dataset: string;
  rows: Row[];
  sort: Sort;
  onSort: (sort: Sort) => void;
}> = (props) => (
  <div class="qd-scroll">
    <table class="table qd-table">
      <thead>
        <tr>
          <For each={COLUMNS}>
            {(key) => (
              <th aria-sort={ariaSort(props.sort, key)}>
                <button
                  type="button"
                  class="qd-sort"
                  onClick={() =>
                    props.onSort({
                      key,
                      descending: props.sort.key === key && !props.sort.descending,
                    })
                  }
                >
                  {t(`quality:rules.col.${key}`)}
                  <Show when={props.sort.key === key}>
                    <span aria-hidden="true">{props.sort.descending ? " ▼" : " ▲"}</span>
                  </Show>
                </button>
              </th>
            )}
          </For>
        </tr>
      </thead>
      <tbody>
        <For each={props.rows}>
          {(r) => (
            <tr>
              <td class="qd-mono">
                <A
                  class="qd-link"
                  title={t("quality:editor.open")}
                  href={`/quality/rules/${encodeURIComponent(props.dataset)}/${encodeURIComponent(r.file)}?rule=${encodeURIComponent(r.id)}`}
                >
                  {r.id}
                </A>
              </td>
              <td>{r.field ?? "—"}</td>
              <td class="qd-mono">{r.pattern}</td>
              <td>{r.dimension}</td>
              <td>{r.severity}</td>
              <td class="qd-wrap">
                <div
                  class="qd-clamp"
                  title={`${r.fails}\n${t("quality:rules.applies")}: ${r.applies}`}
                >
                  {r.fails}
                </div>
              </td>
              <td class="qd-mono qd-hint">{r.file}</td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  </div>
);

/**
 * Rules of a dataset (decision 0019), opened from its ▤ in the tree: every
 * rule `main` has, in words,
 * filtered by text, pattern, dimension and severity; the files whose
 * rules do not lint, named first.
 */
export const RulesPage: Component = () => {
  // The dataset shown, from its ▤ in the tree (`?dataset=`).
  const [search] = useSearchParams<{ dataset?: string }>();
  const dataset = () => search.dataset ?? "";
  const [filter, setFilter] = createSignal<Filter>(NO_FILTER);
  const [sort, setSort] = createSignal<Sort>(BY_ID);
  // Asked again when a file is deployed, imported or restored (0051).
  const [rules] = createResource(
    () => (dataset() ? { dataset: dataset(), n: ruled() } : undefined),
    (k) => rulesApi.read(k.dataset),
  );
  createEffect(on(dataset, () => setFilter(NO_FILTER), { defer: true }));
  const all = () => (rules() ? rows(rules()!) : []);
  const shown = () => sorted(keep(all(), filter()), sort());

  return (
    <section class="feature qd-page">
      <header class="qd-head">
        <div>
          <h1>
            {t("quality:rules.title")}{" "}
            <Show when={dataset()}>
              <span class="qd-mono">{dataset()}</span>
            </Show>
          </h1>
          <p class="qd-hint">{t("quality:rules.subtitle")}</p>
        </div>
        <Show when={rules()}>{(out) => <RulesIO dataset={dataset()} rules={out()} />}</Show>
      </header>
      <LoadFailed when={rules.error} />
      <Show when={!dataset()}>
        <div class="card qd-nothing">
          <p class="qd-nothing__title">{t("quality:rules.nothing")}</p>
          <p class="qd-hint">{t("quality:rules.choose")}</p>
        </div>
      </Show>
      <Show when={rules()}>
        {(out) => (
          <div class="card">
            <Show when={!out().mapping_reads}>
              <p class="qd-error">{t("quality:rules.mappingBroken")}</p>
            </Show>
            <For each={broken(out())}>
              {(b) => (
                <p class="qd-file-errors">
                  <Status
                    tone="critical"
                    label={t("quality:drafts.errors", { count: b.errors.length })}
                  />
                  <span class="qd-mono">{b.file}</span>
                  <span class="qd-hint">{b.errors.map((e) => e.message).join(" · ")}</span>
                </p>
              )}
            </For>
            <Show
              when={all().length > 0}
              fallback={<p class="qd-hint">{t("quality:rules.none", { dataset: dataset() })}</p>}
            >
              <RulesFilters rows={all()} filter={filter()} onChange={setFilter} />
              <p class="qd-hint">
                {t("quality:rules.count", { count: shown().length, total: all().length })}
              </p>
              <Show
                when={shown().length > 0}
                fallback={<p class="qd-hint">{t("quality:rules.empty")}</p>}
              >
                <RulesTable dataset={dataset()} rows={shown()} sort={sort()} onSort={setSort} />
              </Show>
            </Show>
          </div>
        )}
      </Show>
    </section>
  );
};
