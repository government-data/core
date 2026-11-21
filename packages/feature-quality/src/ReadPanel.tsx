import { For, Match, Show, Switch, type Component } from "solid-js";
import { Spiral } from "@knime-orchestador/feature-kit/ui";
import type { CheckOut, FindingOut, RuleView } from "./api";
import { isStyle } from "./editor";
import { LintIcon } from "./LintIcon";
import { t } from "./i18n";
import type { Columns } from "./ColumnsPicker";
import { ResultTab, type TestReveal } from "./ResultTab";
import type { TestState } from "./ruleTest";

/** A finding, with the line it is on; a click goes there. */
const FindingRow: Component<{ f: FindingOut; onGo: (line: number) => void }> = (props) => (
  <li>
    <button
      type="button"
      class="qd-finding"
      disabled={!props.f.line}
      onClick={() => props.f.line && props.onGo(props.f.line)}
    >
      <LintIcon
        severity={props.f.severity === "error" ? "error" : "warning"}
        label={t(props.f.severity === "error" ? "quality:editor.error" : "quality:editor.warning")}
      />
      <span class="qd-finding__message">{props.f.message}</span>
      <span class="qd-mono qd-code">{props.f.code}</span>
      <Show when={props.f.line}>
        {(line) => <span class="qd-code">{t("quality:editor.line", { line: line() })}</span>}
      </Show>
    </button>
  </li>
);

const Findings: Component<{
  findings: FindingOut[];
  empty: string;
  onGo: (line: number) => void;
}> = (props) => (
  <Show when={props.findings.length > 0} fallback={<p class="qd-hint">{props.empty}</p>}>
    <ul class="qd-findings">
      <For each={props.findings}>{(f) => <FindingRow f={f} onGo={props.onGo} />}</For>
    </ul>
  </Show>
);

/** Each rule in words (decision 0019, point 9), its SQL per engine folded. */
const RulesRead: Component<{ rules: RuleView[] }> = (props) => (
  <Show
    when={props.rules.length > 0}
    fallback={<p class="qd-hint">{t("quality:editor.noRules")}</p>}
  >
    <For each={props.rules}>
      {(r) => (
        <section class="qd-rule-read">
          <h3>
            <span class="qd-mono">{r.id}</span>{" "}
            <span class="qd-hint">
              {r.kind} · {r.dimension} · {r.severity}
            </span>
          </h3>
          <p>
            <b>{t("quality:rules.applies")}:</b> {r.applies}
          </p>
          <p>
            <b>{t("quality:rules.col.fails")}:</b> {r.fails}
          </p>
          <Show when={r.skips}>
            {(s) => (
              <p>
                <b>{t("quality:editor.skips")}:</b> {s()}
              </p>
            )}
          </Show>
          <details>
            <summary>{t("quality:editor.sql")}</summary>
            <pre class="qd-yaml">{r.duckdb ?? "—"}</pre>
          </details>
        </section>
      )}
    </For>
  </Show>
);

export type Tab = "problems" | "style" | "read" | "result";

/**
 * The panel under the editor, in the same card, in tabs: _Problems_ (lint's errors and
 * warnings on what the rules say), _Style_ (how the file is written,
 * decision 0041), _As read_ (each rule in words) and _Result_ (the last
 * test, decision 0043); a click on a finding goes to its line.
 */
export const ReadPanel: Component<{
  check?: CheckOut;
  checking: boolean;
  test: TestState;
  tab: Tab;
  onTab: (tab: Tab) => void;
  /** Its height in pixels, set by the bar above it. */
  height: number;
  onGo: (line: number) => void;
  /** A tested rule chosen in _Result_. */
  onRule: (rule: string) => void;
  /** The fields a test shows too. */
  columns?: Columns;
  /** A test's values shown whole (decision 0069). */
  reveal?: TestReveal;
}> = (props) => {
  const tab = () => props.tab;
  const setTab = (x: Tab) => props.onTab(x);
  const tested = () =>
    props.test.phase === "done" ? props.test.out.report?.rules.length : undefined;
  const findings = () => props.check?.findings ?? [];
  const problems = () => findings().filter((f) => !isStyle(f.code));
  const style = () => findings().filter((f) => isStyle(f.code));
  const tabs = (): { id: Tab; label: string; count?: number }[] => [
    { id: "problems", label: t("quality:editor.tabs.problems"), count: problems().length },
    { id: "style", label: t("quality:editor.tabs.style"), count: style().length },
    { id: "read", label: t("quality:editor.asRead"), count: props.check?.rules.length },
    { id: "result", label: t("quality:editor.test.tab"), count: tested() },
  ];
  return (
    <section class="qd-panel" style={{ height: `${props.height}px` }}>
      <div class="qd-panel__tabs" role="tablist">
        <For each={tabs()}>
          {(x) => (
            <button
              type="button"
              role="tab"
              class="qd-panel__tab"
              aria-selected={tab() === x.id}
              onClick={() => setTab(x.id)}
            >
              {x.label}
              <Show when={x.count}>
                <span class="qd-panel__count">{x.count}</span>
              </Show>
            </button>
          )}
        </For>
        <Show when={props.checking || props.test.phase === "waiting"}>
          <span class="qd-hint qd-panel__status">
            <Show when={props.test.phase === "waiting"}>
              <Spiral class="qd-spiral-sm" />
            </Show>
            {props.checking ? t("quality:editor.checking") : t("quality:editor.test.testing")}
          </span>
        </Show>
      </div>
      <div class="qd-panel__body" role="tabpanel">
        {/* Kept while hidden: the rule, filter and page chosen stay. */}
        <div hidden={tab() !== "result"}>
          <ResultTab
            test={props.test}
            onRule={props.onRule}
            columns={props.columns}
            reveal={props.reveal}
          />
        </div>
        <Show when={tab() !== "result" && props.check}>
          {(c) => (
            <Switch>
              <Match when={tab() === "problems"}>
                <Findings
                  findings={problems()}
                  empty={t("quality:drafts.noErrors")}
                  onGo={props.onGo}
                />
              </Match>
              <Match when={tab() === "style"}>
                <Findings
                  findings={style()}
                  empty={t("quality:editor.noStyle")}
                  onGo={props.onGo}
                />
              </Match>
              <Match when={tab() === "read"}>
                <RulesRead rules={c().rules} />
              </Match>
            </Switch>
          )}
        </Show>
      </div>
    </section>
  );
};
