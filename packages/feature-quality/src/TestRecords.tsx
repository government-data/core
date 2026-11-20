import { For, Show, type Component } from "solid-js";
import type { TestedRecord } from "./api";
import { t } from "./i18n";
import { page, PAGE } from "./ruleTest";

/** A rule's records read, a page at a time: each one's outcome and its fields. */
export const TestRecords: Component<{
  records: TestedRecord[];
  fields: string[];
  n: number;
  onPage: (n: number) => void;
}> = (props) => (
  <Show
    when={props.records.length > 0}
    fallback={<p class="qd-hint">{t("quality:editor.test.noRecords")}</p>}
  >
    <div class="qd-wide">
      <table class="table qd-table qd-test-table">
        <thead>
          <tr>
            <th>{t("quality:editor.test.outcomeCol")}</th>
            <For each={props.fields}>{(f) => <th class="qd-mono">{f}</th>}</For>
          </tr>
        </thead>
        <tbody>
          <For each={page(props.records, props.n)}>
            {(x) => (
              <tr>
                <td>
                  <span class={`qd-outcome qd-outcome--${x.outcome}`}>
                    {t(`quality:editor.test.outcome.${x.outcome}`)}
                  </span>
                </td>
                <For each={props.fields}>
                  {(f) => {
                    const value = x.values.find((v) => v.field === f)?.value;
                    return (
                      <td class="qd-mono">
                        {value ?? <span class="qd-hint">{t("quality:editor.test.empty")}</span>}
                      </td>
                    );
                  }}
                </For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
    <Show when={props.records.length > PAGE}>
      <div class="qd-test-pager">
        <button
          type="button"
          class="fk-btn"
          disabled={props.n === 0}
          onClick={() => props.onPage(props.n - 1)}
        >
          {t("quality:editor.test.previous")}
        </button>
        <span class="qd-hint">
          {t("quality:editor.test.page", {
            from: props.n * PAGE + 1,
            to: Math.min((props.n + 1) * PAGE, props.records.length),
            total: props.records.length,
          })}
        </span>
        <button
          type="button"
          class="fk-btn"
          disabled={(props.n + 1) * PAGE >= props.records.length}
          onClick={() => props.onPage(props.n + 1)}
        >
          {t("quality:editor.test.next")}
        </button>
      </div>
    </Show>
  </Show>
);
