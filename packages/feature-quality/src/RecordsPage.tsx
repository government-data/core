import "./quality.css";
import { createResource, createSignal, For, onCleanup, Show, type Component } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { formatNumber } from "@knime-orchestador/i18n";
import { recordsApi, sourcesApi, type RecordsAsk, type RecordsOut } from "./api";
import { askFor, ready, refusal, type Mode } from "./records";
import { t } from "./i18n";
import { masked, reveal, scopes, type Reason, type Revealing } from "./reveal";
import { revealApi } from "./revealApi";
import { RevealBar } from "./RevealDialog";

/** How often a page on its way is asked after. */
const POLL_MS = 1000;
/** The records of a page (`quality::source_records::PAGE`). */
const PAGE = 1000;

/** A page as the screen shows it. */
type Shown =
  | { phase: "idle" }
  | { phase: "waiting" }
  | ({ phase: "done" } & Revealing<RecordsOut>)
  | { phase: "refused"; reason: string };

/** The page's records as a grid; an empty value shown as such, never as text. */
const Grid: Component<{ out: RecordsOut }> = (props) => (
  <Show when={props.out.page}>
    {(page) => (
      <div class="qd-scroll">
        <table class="table qd-table qd-records">
          <thead>
            <tr>
              <For each={page().columns}>{(c) => <th class="qd-mono">{c}</th>}</For>
            </tr>
          </thead>
          <tbody>
            <For
              each={page().rows}
              fallback={
                <tr>
                  <td colSpan={page().columns.length}>{t("quality:records.none")}</td>
                </tr>
              }
            >
              {(row) => (
                <tr>
                  <For each={row}>
                    {(value) => (
                      <td class="qd-mono">
                        <Show when={value !== null} fallback={<em class="qd-hint">∅</em>}>
                          {value}
                        </Show>
                      </td>
                    )}
                  </For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    )}
  </Show>
);

/**
 * A page asked and waited for: `read(ask)` queues it and asks after it
 * until it ends; then shown whole with the reason kept for the source, if
 * any (decision 0069), or with `reason` given.
 */
function createRead(source: () => string) {
  const [shown, setShown] = createSignal<Shown>({ phase: "idle" });
  let timer: ReturnType<typeof setTimeout> | undefined;
  let asked = 0;
  onCleanup(() => clearTimeout(timer));
  const follow = async (mine: number, job: string) => {
    if (mine !== asked) return;
    try {
      const out = await recordsApi.read(source(), job);
      if (mine !== asked) return;
      if (out.state === "done" || out.state === "failed") {
        setShown({ phase: "done", job, masked: out, out });
        void showWhole(mine);
      } else timer = setTimeout(() => void follow(mine, job), POLL_MS);
    } catch (e) {
      if (mine === asked) setShown({ phase: "refused", reason: refusal(e) });
    }
  };
  const read = async (ask: RecordsAsk) => {
    const mine = ++asked;
    clearTimeout(timer);
    setShown({ phase: "waiting" });
    try {
      const { job } = await recordsApi.ask(source(), ask);
      void follow(mine, job);
    } catch (e) {
      if (mine === asked) setShown({ phase: "refused", reason: refusal(e) });
    }
  };
  const showWhole = async (mine: number, reason?: Reason) => {
    const s = shown();
    if (s.phase !== "done") return;
    const post = (r: Reason) => revealApi.records(source(), s.job, r);
    await reveal(
      scopes.source(source()),
      s,
      post,
      (v) => {
        if (mine === asked) setShown({ phase: "done", ...v });
      },
      reason,
    );
  };
  const mask = () => {
    const s = shown();
    if (s.phase === "done") setShown({ phase: "done", ...masked(s) });
  };
  return { shown, read, showWhole: (reason: Reason) => void showWhole(asked, reason), mask };
}

/** How to read: the first, the last or every page, the order, and _Read_. */
const Controls: Component<{
  columns: string[];
  mode: Mode;
  order: string;
  waiting: boolean;
  onMode: (m: Mode) => void;
  onOrder: (c: string) => void;
  onRead: () => void;
}> = (props) => (
  <div class="card qd-records__bar">
    <label class="qd-field">
      <span>{t("quality:records.mode.label")}</span>
      <select value={props.mode} onChange={(e) => props.onMode(e.currentTarget.value as Mode)}>
        <option value="first">{t("quality:records.mode.first")}</option>
        <option value="last">{t("quality:records.mode.last")}</option>
        <option value="all">{t("quality:records.mode.all")}</option>
      </select>
    </label>
    <label class="qd-field">
      <span>{t("quality:records.order")}</span>
      <select value={props.order} onChange={(e) => props.onOrder(e.currentTarget.value)}>
        <option value="">{t("quality:records.noOrder")}</option>
        <For each={props.columns}>{(c) => <option value={c}>{c}</option>}</For>
      </select>
    </label>
    <button
      type="button"
      class="fk-btn fk-btn--primary"
      disabled={!ready(props.mode, props.order) || props.waiting}
      onClick={() => props.onRead()}
    >
      {t("quality:records.read")}
    </button>
    <Show when={props.mode === "last" && !props.order}>
      <small class="qd-hint">{t("quality:records.lastNeedsOrder")}</small>
    </Show>
  </div>
);

/** A page read: masked or not, which records, the grid and, reading all, the pages. */
const Result: Component<{
  source: string;
  view: Revealing<RecordsOut>;
  out: RecordsOut;
  onReveal: (reason: Reason) => void;
  onMask: () => void;
  page: number;
  pages: boolean;
  onPage: (n: number) => void;
}> = (props) => (
  <div class="card">
    <Show when={props.out.state === "failed"}>
      <p class="qd-error">{t("quality:records.failed")}</p>
    </Show>
    <Show when={props.out.masked && !props.out.revealable && !!props.out.page?.rows.length}>
      <p class="qd-hint">{t("quality:records.masked")}</p>
    </Show>
    <RevealBar
      scope={scopes.source(props.source)}
      revealable={props.view.masked.revealable}
      masked={props.out.masked}
      busy={props.view.busy}
      refused={props.view.refused}
      onReveal={props.onReveal}
      onMask={props.onMask}
    />
    <Show when={props.out.page}>
      {(p) => (
        <p class="qd-hint">
          {t("quality:records.shown", {
            from: formatNumber(props.page * PAGE + (p().rows.length ? 1 : 0)),
            to: formatNumber(props.page * PAGE + p().rows.length),
          })}
        </p>
      )}
    </Show>
    <Grid out={props.out} />
    <Show when={props.pages}>
      <div class="qd-records__pages">
        <button
          type="button"
          class="fk-btn fk-btn--sm"
          disabled={props.page === 0}
          onClick={() => props.onPage(props.page - 1)}
        >
          {t("quality:records.previous")}
        </button>
        <button
          type="button"
          class="fk-btn fk-btn--sm"
          disabled={!props.out.page?.more}
          onClick={() => props.onPage(props.page + 1)}
        >
          {t("quality:records.next")}
        </button>
      </div>
    </Show>
  </div>
);

/**
 * A source table's records (decision 0049, point 3), as a database client
 * shows them: the first ones, the last ones by a column, or every page in
 * an order, a page of 1,000 at a time; masked without permission to see
 * personal data, every read with values audited by the API.
 */
export const RecordsPage: Component = () => {
  const [search] = useSearchParams<{ source?: string; schema?: string; table?: string }>();
  const source = () => search.source ?? "";
  const table = () => `${search.schema ?? ""}.${search.table ?? ""}`;
  const [listing] = createResource(source, (s) => sourcesApi.tables(s));
  const columns = () =>
    listing()
      ?.tables.find((x) => x.schema === search.schema && x.name === search.table)
      ?.columns.map((c) => c.name) ?? [];
  const [mode, setMode] = createSignal<Mode>("first");
  const [order, setOrder] = createSignal("");
  const [page, setPage] = createSignal(0);
  const { shown, read, showWhole, mask } = createRead(source);
  const readAt = (at: number) => {
    setPage(at);
    void read(askFor(table(), mode(), order(), at));
  };
  const done = () => {
    const s = shown();
    return s.phase === "done" ? s : undefined;
  };
  const refused = () => {
    const s = shown();
    return s.phase === "refused" ? s.reason : undefined;
  };
  return (
    <section class="feature qd-page">
      <header class="qd-head">
        <div>
          <h1 class="qd-mono">{table()}</h1>
          <p class="qd-hint">{t("quality:records.subtitle", { source: source() })}</p>
        </div>
      </header>
      <Controls
        columns={columns()}
        mode={mode()}
        order={order()}
        waiting={shown().phase === "waiting"}
        onMode={setMode}
        onOrder={setOrder}
        onRead={() => readAt(0)}
      />
      <Show when={shown().phase === "waiting"}>
        <p class="qd-hint">{t("quality:records.waiting")}</p>
      </Show>
      <Show when={refused()}>
        {(why) => (
          <p class="qd-error">
            {t(`quality:records.refused.${why()}`, {
              defaultValue: t("quality:records.refused.other"),
            })}
          </p>
        )}
      </Show>
      <Show when={done()}>
        {(view) => (
          <Result
            source={source()}
            view={view()}
            out={view().out}
            page={page()}
            pages={mode() === "all"}
            onPage={readAt}
            onReveal={showWhole}
            onMask={mask}
          />
        )}
      </Show>
    </section>
  );
};
