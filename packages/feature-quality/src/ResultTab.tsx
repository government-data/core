import { createSignal, For, Match, onCleanup, Show, Switch, type Component } from "solid-js";
import { Spiral } from "@knime-orchestador/feature-kit/ui";
import type { RuleTested } from "./api";
import { t } from "./i18n";
import { ColumnsPicker, type Columns } from "./ColumnsPicker";
import { FailGroups } from "./FailGroups";
import { TestRecords } from "./TestRecords";
import { OutcomeIcon, type Outcome } from "./TestIcons";
import { columns, conformity, hasValues, PAGE, type TestState } from "./ruleTest";
import type { Reason } from "./reveal";
import { RevealBar } from "./RevealDialog";

/** Showing a test's values whole (decision 0069): the dataset's scope, and how. */
export interface TestReveal {
  scope: string;
  onReveal: (reason: Reason) => void;
  onMask: () => void;
}

const OUTCOMES: Outcome[] = ["fails", "passes", "skips"];

/** A rule's counts on the cut, conformity last. */
const Counts: Component<{ r: RuleTested }> = (props) => (
  <span class="qd-test-counts">
    <span>
      {t("quality:editor.test.applies")} <b>{props.r.applies}</b>
    </span>
    <For each={OUTCOMES}>
      {(o) => (
        <span>
          {t(`quality:editor.test.outcome.${o}`)} <b>{props.r[o]}</b>
        </span>
      )}
    </For>
    <span>
      {t("quality:editor.test.conformity")} <b>{conformity(props.r)?.toLocaleString() ?? "—"}</b>
      <Show when={conformity(props.r) !== undefined}>%</Show>
    </span>
  </span>
);

/** The rule a test opens on: the first that fails, or the first. */
export const firstShown = (rules: RuleTested[]) => rules.find((r) => r.fails > 0) ?? rules[0];

/** The outcomes to filter a rule's records by, as icons: every one, fails, passes, does not apply. */
const OutcomeFilter: Component<{
  r: RuleTested;
  only?: Outcome;
  onChoose: (o?: Outcome) => void;
}> = (props) => (
  <>
    <button
      type="button"
      class="fk-btn qd-icon-btn"
      aria-pressed={!props.only}
      title={t("quality:editor.test.all")}
      aria-label={t("quality:editor.test.all")}
      onClick={() => props.onChoose()}
    >
      <OutcomeIcon />
    </button>
    <For each={OUTCOMES}>
      {(o) => (
        <button
          type="button"
          class={`fk-btn qd-icon-btn qd-icon-btn--${o}`}
          aria-pressed={props.only === o}
          title={`${t(`quality:editor.test.outcome.${o}`)} (${props.r[o]})`}
          aria-label={t(`quality:editor.test.outcome.${o}`)}
          onClick={() => props.onChoose(o)}
        >
          <OutcomeIcon of={o} />
        </button>
      )}
    </For>
  </>
);

/** One rule's result: its counts, then its records, by outcome, a page at a time. */
const RuleResult: Component<{ r: RuleTested; columns?: Columns }> = (props) => {
  const [only, setOnly] = createSignal<Outcome | undefined>(
    props.r.fails > 0 ? "fails" : undefined,
  );
  const [n, setN] = createSignal(0);
  const [group, setGroup] = createSignal<string>();
  const grouped = () => only() === "fails" && (props.r.groups ?? []).length > 0;
  const records = () => {
    const g = grouped() && (props.r.groups ?? []).find((x) => x.shape === group());
    return g ? g.records : props.r.records.filter((x) => !only() || x.outcome === only());
  };
  const fields = () => columns(props.r.records);
  const choose = (o?: Outcome) => {
    setOnly(o);
    setGroup(undefined);
    setN(0);
  };
  return (
    <div class="qd-test-rule">
      <div class="qd-test-head">
        <span class="qd-mono">{props.r.rule}</span>
        <Counts r={props.r} />
      </div>
      <div class="qd-test-filter">
        <span class="qd-hint">{t("quality:editor.test.sampled", { each: PAGE })}</span>
        <span class="qd-test-filter__icons" role="group">
          <OutcomeFilter r={props.r} only={only()} onChoose={choose} />
          <Show when={props.columns}>{(c) => <ColumnsPicker {...c()} />}</Show>
        </span>
      </div>
      <Show when={grouped()}>
        <FailGroups
          r={props.r}
          chosen={group()}
          onChoose={(shape) => {
            setGroup(shape);
            setN(0);
          }}
        />
      </Show>
      <TestRecords records={records()} fields={fields()} n={n()} onPage={setN} />
    </div>
  );
};

/** The rules tested, one chosen at a time: its records beside the list. */
const Rules: Component<{
  rules: RuleTested[];
  onRule: (rule: string) => void;
  columns?: Columns;
}> = (props) => {
  const [chosen, setChosen] = createSignal(firstShown(props.rules).rule);
  const rule = () => props.rules.find((r) => r.rule === chosen()) ?? props.rules[0];
  return (
    <div class="qd-test-layout" classList={{ "qd-test-layout--one": props.rules.length === 1 }}>
      <Show when={props.rules.length > 1}>
        <ul class="qd-test-rules" role="listbox" aria-label={t("quality:editor.test.rules")}>
          <For each={props.rules}>
            {(r) => (
              <li>
                <button
                  type="button"
                  role="option"
                  class="qd-test-choice"
                  aria-selected={r.rule === chosen()}
                  onClick={() => {
                    setChosen(r.rule);
                    props.onRule(r.rule);
                  }}
                >
                  <span class="qd-mono">{r.rule}</span>
                  <span
                    class={
                      r.fails > 0 ? "qd-outcome qd-outcome--fails" : "qd-outcome qd-outcome--passes"
                    }
                  >
                    {r.fails}
                  </span>
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={rule()} keyed>
        {(r) => <RuleResult r={r} columns={props.columns} />}
      </Show>
    </div>
  );
};

/** The seconds since `since`, counted each second while shown. */
function elapsed(since: () => number) {
  const [now, setNow] = createSignal(Date.now());
  const tick = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(tick));
  return () => Math.max(0, Math.round((now() - since()) / 1000));
}

/** A test on its way: what it is waiting for or doing, and for how long. */
const Waiting: Component<{ test: Extract<TestState, { phase: "waiting" }> }> = (props) => {
  const seconds = elapsed(() => props.test.since);
  const running = () => props.test.state === "running";
  const what = () => props.test.rule ?? t("quality:editor.test.theFile");
  return (
    <div class="qd-test-waiting" role="status">
      <Spiral class="qd-test-spiral" />
      <div>
        <p>
          {running()
            ? t("quality:editor.test.running", { what: what(), cut: props.test.cut ?? "" })
            : t("quality:editor.test.queued", { what: what() })}
          <span class="qd-hint"> · {t("quality:editor.test.elapsed", { seconds: seconds() })}</span>
        </p>
        <p class="qd-hint">
          {running() ? t("quality:editor.test.runningHint") : t("quality:editor.test.queuedHint")}
        </p>
      </div>
    </div>
  );
};

/** What a finished test found, or why it did not finish. */
const Found: Component<{
  test: Extract<TestState, { phase: "done" }>;
  onRule: (rule: string) => void;
  columns?: Columns;
  reveal?: TestReveal;
}> = (props) => {
  const out = () => props.test.out;
  return (
    <Show
      when={out().report}
      fallback={
        <p class="qd-error" role="alert">
          {t("quality:editor.test.failed")} <span class="qd-mono qd-hint">{out().error ?? ""}</span>
        </p>
      }
    >
      {(report) => (
        <>
          <p class="qd-hint">
            {t("quality:editor.test.read", { records: report().records, cut: report().cut })}
            <Show when={!out().masked || (!props.test.masked.revealable && hasValues(report()))}>
              {" · "}
              {out().masked ? t("quality:editor.test.masked") : t("quality:editor.test.seen")}
            </Show>
          </p>
          <Show when={props.reveal}>
            {(r) => (
              <RevealBar
                scope={r().scope}
                revealable={props.test.masked.revealable}
                masked={out().masked}
                busy={props.test.busy}
                refused={props.test.refused}
                onReveal={r().onReveal}
                onMask={r().onMask}
              />
            )}
          </Show>
          <Show
            when={report().rules.length > 0}
            fallback={<p class="qd-hint">{t("quality:editor.test.noRule")}</p>}
          >
            <Rules rules={report().rules} onRule={props.onRule} columns={props.columns} />
          </Show>
        </>
      )}
    </Show>
  );
};

/**
 * _Result_ (decision 0043, point 3): the last test of the text as typed,
 * while it runs, and then each rule's counts and a read of its records.
 */
export const ResultTab: Component<{
  test: TestState;
  /** A rule chosen: the editor goes to its line. */
  onRule: (rule: string) => void;
  /** The fields to show too, chosen beside the result. */
  columns?: Columns;
  /** Its values shown whole, for whoever may. */
  reveal?: TestReveal;
}> = (props) => (
  <Switch>
    <Match when={props.test.phase === "idle"}>
      <p class="qd-hint">{t("quality:editor.test.idle")}</p>
    </Match>
    <Match when={props.test.phase === "waiting" && props.test}>
      {(w) => <Waiting test={w()} />}
    </Match>
    <Match when={props.test.phase === "refused" && props.test}>
      {(r) => (
        <p class="qd-error" role="alert">
          {t(`quality:editor.test.refused.${r().reason}`)}
        </p>
      )}
    </Match>
    <Match when={props.test.phase === "expired"}>
      <p class="qd-hint" role="status">
        {t("quality:editor.test.expired")}
      </p>
    </Match>
    <Match when={props.test.phase === "done" && props.test}>
      {(d) => (
        <Found test={d()} onRule={props.onRule} columns={props.columns} reveal={props.reveal} />
      )}
    </Match>
  </Switch>
);
