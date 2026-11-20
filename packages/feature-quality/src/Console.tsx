import {
  createResource,
  createSignal,
  For,
  Match,
  onCleanup,
  Show,
  Switch,
  type Component,
} from "solid-js";
import { Modal, Spiral } from "@knime-orchestador/feature-kit/ui";
import { lakeApi, may, type DatasetOut } from "./api";
import {
  asked,
  current,
  draft,
  forget,
  runStatement,
  setDraft,
  showStatement,
  shown,
  type Asked,
} from "./consoleStore";
import { knownMe, who } from "./explorerCache";
import type { MenuItem } from "./TreeMenu";
import { t } from "./i18n";
import { warn } from "./Notify";
import { ConsoleRows } from "./ConsoleRows";
import { SqlEditor } from "./SqlEditor";
import { formatted, schemaQuery, tableQuery } from "./consoleSql";

/** The lake's last listing, as completion takes it: columns by `lake.<namespace>.<table>`. */
async function lakeSchema(): Promise<Record<string, string[]>> {
  const listing = await lakeApi.tables().catch(() => undefined);
  return Object.fromEntries(
    (listing?.tables ?? []).map((x) => [
      `lake.${x.schema}.${x.name}`,
      x.columns.map((c) => c.name),
    ]),
  );
}

/**
 * The console (decision 0054): one `SELECT` over the tenant's lake, run on
 * a worker; its rows, at most 200, masked for whoever may not see personal
 * data. _Run_ or Ctrl+Enter.
 */
const ConsoleBody: Component = () => {
  const [tables] = createResource(lakeSchema);
  const tidy = () => {
    const tidied = formatted(draft());
    if (tidied === undefined) warn(t("quality:console.unformattable"));
    else setDraft(tidied);
  };
  const running = () => current()?.ran.phase === "running";
  const run = () => !running() && draft().trim() && void runStatement(draft());
  return (
    <div class="qd-console">
      <SqlEditor
        text={draft()}
        tables={tables() ?? {}}
        label={t("quality:console.statement")}
        onChange={setDraft}
        onRun={run}
      />
      <div class="qd-actions">
        <Show when={running()}>
          <span class="qd-hint">
            <Spiral class="qd-spiral-sm" /> {t("quality:console.running")}
          </span>
        </Show>
        <button
          type="button"
          class="fk-btn"
          disabled={!draft().trim()}
          title={t("quality:console.formatHint")}
          onClick={tidy}
        >
          {t("quality:console.format")}
        </button>
        <button
          type="button"
          class="fk-btn fk-btn--primary"
          disabled={running() || !draft().trim()}
          title={t("quality:console.keys")}
          onClick={run}
        >
          {t("quality:console.run")}
        </button>
      </div>
      <Kept />
      <Show when={current()?.ran} keyed>
        {(ran) => (
          <Switch>
            <Match when={ran.phase === "refused"}>
              <p class="qd-error" role="alert">
                {t("quality:console.refused")}
              </p>
            </Match>
            <Match when={ran.phase === "done" && ran}>
              {(view) => <ConsoleRows id={current()?.id ?? 0} view={view()} />}
            </Match>
          </Switch>
        )}
      </Show>
    </div>
  );
};

/** The console in a dialog, where no editor is beside it. */
const ConsolePanel: Component<{ onClose: () => void }> = (props) => (
  <Modal open size="wide" title={t("quality:console.title")} onClose={props.onClose}>
    <div class="qd-modal-scroll">
      <ConsoleBody />
    </div>
  </Modal>
);

/** What came of a statement, in a word: rows, failed, or running. */
const outcome = (a: Asked) =>
  a.ran.phase === "running"
    ? t("quality:console.running")
    : a.ran.phase === "refused" || a.ran.out.result?.error || a.ran.out.state === "failed"
      ? t("quality:console.failed")
      : t("quality:console.rows", { count: a.ran.out.result?.rows.length ?? 0 });

/** The statements run while the page is open, newest first: one shown again with a click. */
const Kept: Component = () => (
  <Show when={asked().length > 0}>
    <details class="qd-console__kept" open>
      <summary>{t("quality:console.kept", { count: asked().length })}</summary>
      <ul>
        <For each={asked()}>
          {(a) => (
            <li classList={{ "qd-console__kept--shown": a.id === shown() }}>
              <button
                type="button"
                class="qd-link qd-mono"
                title={a.sql}
                onClick={() => showStatement(a.id)}
              >
                {a.sql.replace(/\s+/g, " ").slice(0, 90)}
              </button>{" "}
              <small class="qd-hint">
                {a.at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} · {outcome(a)}
              </small>{" "}
              <button
                type="button"
                class="qd-link"
                aria-label={t("quality:console.forget")}
                title={t("quality:console.forget")}
                onClick={() => forget(a.id)}
              >
                ×
              </button>
            </li>
          )}
        </For>
      </ul>
    </details>
  </Show>
);

/** Where a dataset's records are in the lake: its bronze table, if it has one. */
export const bronzeOf = (d: DatasetOut): [string, string] | undefined =>
  d.bronze ? [`${d.name}_bronze`, d.bronze] : undefined;

/**
 * A lake table's menu (decision 0054): _Run a query_, which opens the
 * console on its records, for whoever may query the lake.
 */
export function queryMenu(table: [string, string] | undefined): MenuItem[] {
  if (!table || !may(knownMe(), "quality:QueryData")) return [];
  return [{ label: t("quality:console.onTable"), run: () => openConsole(tableQuery(...table)) }];
}

/**
 * A lake namespace's menu: _Run a query_ on its one table, or on the list
 * of its tables when it has several.
 */
export function schemaMenu(namespace: string, tables: { name: string }[]): MenuItem[] {
  if (tables.length === 1) return queryMenu([namespace, tables[0].name]);
  if (!may(knownMe(), "quality:QueryData")) return [];
  return [{ label: t("quality:console.onTable"), run: () => openConsole(schemaQuery(namespace)) }];
}

/** The console in a dialog, opened from the tree where no editor is shown. */
const [opened, setOpened] = createSignal(false);
/** The console beside the rules' editor, to compare a rule and a query. */
const [docked, setDocked] = createSignal(false);
export { docked };
/** How many editors there are to dock it beside (one, or none). */
let docks = 0;

/** The console opened with `sql` proposed, not run: beside the editor if one is shown. */
export const openConsole = (sql: string) => {
  setDraft(sql);
  if (docks > 0) setDocked(true);
  else setOpened(true);
};

/**
 * The console as the editor's second column (decision 0054): the rule
 * and its result on one side, a query and its rows on the other.
 */
export const ConsoleDock: Component = () => {
  docks++;
  onCleanup(() => docks--);
  return (
    <Show when={docked()}>
      <aside class="card qd-console-dock" aria-label={t("quality:console.title")}>
        <header class="qd-console-dock__head">
          <h2 class="qd-section">{t("quality:console.title")}</h2>
          <button
            type="button"
            class="fk-btn fk-btn--sm"
            aria-label={t("quality:console.close")}
            title={t("quality:console.close")}
            onClick={() => setDocked(false)}
          >
            ×
          </button>
        </header>
        <ConsoleBody />
      </aside>
    </Show>
  );
};

/** The console opened by `openConsole`, once, outside the tree that asked it. */
export const ConsoleHost: Component = () => (
  <Show when={opened()}>
    <ConsolePanel onClose={() => setOpened(false)} />
  </Show>
);

/** `›_` in the editor's bar, for whoever may query the lake: the console beside it, or not. */
export const ConsoleButton: Component = () => {
  const [me] = createResource(who);
  return (
    <Show when={may(me(), "quality:QueryData")}>
      <button
        type="button"
        class="fk-btn qd-console-button"
        aria-pressed={docked()}
        title={t("quality:console.title")}
        aria-label={t("quality:console.title")}
        onClick={() => setDocked(!docked())}
      >
        ›_
      </button>
    </Show>
  );
};
