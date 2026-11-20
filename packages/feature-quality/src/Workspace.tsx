import { createEffect, createSignal, on, Show, type Component } from "solid-js";
import type { createChecker } from "./checker";
import { marks, ruleLine } from "./editor";
import { t } from "./i18n";
import { ReadPanel, type Tab } from "./ReadPanel";
import { ColumnSplitter, createColumnShare } from "./ColumnSplitter";
import { ConsoleButton, ConsoleDock, docked } from "./Console";
import { createRuleTest, ruleAt } from "./ruleTest";
import { scopes } from "./reveal";
import { chooseColumns, columnsOf } from "./testColumns";
import type { createTidy } from "./tidy";
import { createPanelHeight, Splitter } from "./Splitter";
import { extendsNotes } from "./extending";
import { YamlEditor } from "./YamlEditor";

/** The panel's height for a test's records, unless it was made taller. */
const RESULT_PX = 420;

/**
 * The editor and its panel in one card, a bar between them to give one
 * more room and the other less (decision 0019, _Rule_); _Test_ runs the
 * text as typed and shows what it found in _Result_ (0043).
 */
export const Workspace: Component<{
  dataset: string;
  file: string;
  shown: string;
  text: string;
  line?: number;
  schema?: Record<string, unknown>;
  checker: ReturnType<typeof createChecker>;
  tidy: ReturnType<typeof createTidy>;
  onChange: (yaml: string) => void;
  onGo: (line: number) => void;
}> = (props) => {
  const panel = createPanelHeight();
  const columns = createColumnShare();
  let compare: HTMLDivElement | undefined;
  const [cursor, setCursor] = createSignal(1);
  const tester = createRuleTest(
    () => props.dataset,
    () => props.file,
  );
  // Back on a file tested before: its result first.
  const [tab, setTab] = createSignal<Tab>(tester.known ? "result" : "problems");
  const rule = () => ruleAt(props.text, cursor());
  const runTest = () => {
    setTab("result");
    void tester.run(props.text, rule(), columnsOf(props.dataset));
  };
  let box: HTMLDivElement | undefined;
  // A result arrived: room for its records, never less than was chosen.
  createEffect(
    on(tester.test, (x) => {
      const room = Math.min(RESULT_PX, (box?.clientHeight ?? 600) - 160);
      if (x.phase === "done" && panel.height() < room) panel.set(room);
    }),
  );
  return (
    <div
      class="qd-compare"
      ref={compare}
      style={
        docked()
          ? {
              "grid-template-columns": `minmax(0, ${columns.share()}fr) auto minmax(0, ${1 - columns.share()}fr)`,
            }
          : undefined
      }
    >
      <div class="card qd-editing" ref={box}>
        <YamlEditor
          text={props.shown}
          marks={marks(props.text, props.checker.check()?.findings ?? [])}
          line={props.line}
          schema={props.schema}
          fields={props.checker.check()?.fields ?? []}
          notes={extendsNotes(props.checker.check()?.rules ?? [])}
          onTidy={props.tidy.run}
          canTidy={props.tidy.can()}
          tidyNote={props.tidy.stuck() ? t("quality:editor.tidyStuck") : undefined}
          testing={{
            run: runTest,
            can: !props.checker.blocked() && !tester.running(),
            rule: rule(),
          }}
          tools={<ConsoleButton />}
          onChange={props.onChange}
          onCursor={setCursor}
        />
        <Splitter
          height={panel.height()}
          max={() => (box?.clientHeight ?? 600) - 160}
          label={t("quality:editor.resize")}
          onResize={panel.set}
        />
        <ReadPanel
          check={props.checker.check()}
          checking={props.checker.checking()}
          test={tester.test()}
          reveal={{
            scope: scopes.dataset(props.dataset),
            onReveal: tester.showWhole,
            onMask: tester.mask,
          }}
          tab={tab()}
          onTab={setTab}
          height={panel.height()}
          onGo={props.onGo}
          columns={{
            fields: props.checker.check()?.fields ?? [],
            chosen: columnsOf(props.dataset),
            onChoose: (fields) => {
              chooseColumns(props.dataset, fields);
              runTest();
            },
          }}
          onRule={(id) => {
            // A rule a list gave several of (`R-01.Field`) starts on its own line.
            const at = ruleLine(props.text, id) ?? ruleLine(props.text, id.split(".")[0]);
            if (at) props.onGo(at);
          }}
        />
      </div>
      <Show when={docked()}>
        <ColumnSplitter
          share={columns.share()}
          box={() => compare}
          label={t("quality:console.resize")}
          onResize={columns.set}
        />
      </Show>
      <ConsoleDock />
    </div>
  );
};
