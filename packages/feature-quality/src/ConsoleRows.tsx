import { For, Show, type Component } from "solid-js";
import type { ConsoleOut } from "./api";
import { ColumnsPicker } from "./ColumnsPicker";
import { columnsShown, maskAgain, showColumns, showWhole } from "./consoleStore";
import { t } from "./i18n";
import { scopes, type Revealing } from "./reveal";
import { RevealBar } from "./RevealDialog";

/**
 * The rows of a statement (decision 0054), or the engine's words when it
 * failed; _Columns_ chooses which of its columns are shown, without
 * running it again, kept with the statement.
 */
export const ConsoleRows: Component<{ id: number; view: Revealing<ConsoleOut> }> = (props) => {
  const out = () => props.view.out;
  const result = () => out().result;
  const failed = () => result()?.error ?? (out().state === "failed" ? out().error : null);
  return (
    <>
      <RevealBar
        scope={scopes.lake}
        revealable={props.view.masked.revealable}
        masked={out().masked}
        busy={props.view.busy}
        refused={props.view.refused}
        onReveal={(reason) => void showWhole(props.id, reason)}
        onMask={() => maskAgain(props.id)}
      />
      <Rows id={props.id} out={out()} failed={failed()} />
    </>
  );
};

/** A result's rows, the columns chosen, or the engine's words. */
const Rows: Component<{ id: number; out: ConsoleOut; failed?: string | null }> = (props) => {
  const result = () => props.out.result;
  const failed = () => props.failed;
  const all = () => result()?.columns ?? [];
  const shown = () => columnsShown(props.id) ?? all();
  // Where each shown column is in a row.
  const at = () =>
    shown()
      .map((c) => all().indexOf(c))
      .filter((i) => i >= 0);
  return (
    <Show
      when={!failed()}
      fallback={
        <p class="qd-error" role="alert">
          {props.out.masked ? t(`quality:console.kind.${failed() ?? "engine"}`) : failed()}
        </p>
      }
    >
      <p class="qd-hint">
        {t("quality:console.rows", { count: result()?.rows.length ?? 0 })}
        {result()?.more ? ` · ${t("quality:console.more")}` : ""} ·{" "}
        <Show when={!props.out.masked || (!props.out.revealable && !!result()?.rows.length)}>
          {props.out.masked ? t("quality:console.masked") : t("quality:console.seen")}{" "}
        </Show>
        <Show when={all().length > 1}>
          <ColumnsPicker
            fields={all()}
            chosen={shown()}
            label={t("quality:console.columns", { shown: at().length, all: all().length })}
            hint={t("quality:console.columnsHint")}
            apply={t("quality:console.columnsApply")}
            onChoose={(cols) =>
              showColumns(props.id, cols.length === all().length ? undefined : cols)
            }
          />
        </Show>
      </p>
      <div class="qd-console__rows qd-wide">
        <table class="table qd-table">
          <thead>
            <tr>
              <For each={at()}>{(i) => <th class="qd-mono">{all()[i]}</th>}</For>
            </tr>
          </thead>
          <tbody>
            <For each={result()?.rows ?? []}>
              {(row) => (
                <tr>
                  <For each={at()}>
                    {(i) => (
                      <td class="qd-mono">
                        {row[i] ?? <span class="qd-hint">{t("quality:console.null")}</span>}
                      </td>
                    )}
                  </For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
};
