import { createSignal, For, Show, type Component, type JSX } from "solid-js";
import {
  counts,
  hunks,
  inline,
  lineDiff,
  sideBySide,
  type Line,
  type Part,
  type Row,
} from "./diff";
import type { FileChange } from "./api";
import { t } from "./i18n";

export type DiffMode = "split" | "unified";

const MODE_KEPT = "qd.diff.mode";

function keptMode(): DiffMode {
  try {
    return localStorage.getItem(MODE_KEPT) === "unified" ? "unified" : "split";
  } catch {
    return "split";
  }
}

/** Two copies side by side, or one: the same in every diff, kept between visits. */
const [diffMode, setDiffMode] = createSignal<DiffMode>(keptMode());
export { diffMode };

/** _Side by side_ or _Unified_, for every diff at once. */
export const DiffModes: Component = () => (
  <div class="qd-diff__modes" role="group" aria-label={t("quality:drafts.history.view")}>
    <For each={["split", "unified"] as DiffMode[]}>
      {(m) => (
        <button
          type="button"
          class="qd-diff__mode"
          aria-pressed={diffMode() === m}
          onClick={() => {
            setDiffMode(m);
            try {
              localStorage.setItem(MODE_KEPT, m);
            } catch {
              // Without storage the choice lasts until the page is left.
            }
          }}
        >
          {t(`quality:drafts.history.${m}`)}
        </button>
      )}
    </For>
  </div>
);

/** A line's text, what changed in it marked. */
const Text: Component<{ parts?: Part[]; text: string }> = (props) => (
  <Show when={props.parts} fallback={props.text}>
    {(parts) => (
      <For each={parts()}>
        {(p) => (p.changed ? <mark class="qd-diff__mark">{p.text}</mark> : p.text)}
      </For>
    )}
  </Show>
);

/** One side of a row: its line number and text, or an empty cell. */
function side(line: Line | undefined, parts: Part[] | undefined, kind: "del" | "add"): JSX.Element {
  const cls = line && line.kind !== "same" ? `qd-diff__${kind}` : line ? "" : "qd-diff__none";
  return (
    <>
      <td class={`qd-diff__no ${cls}`}>
        {line ? (kind === "del" ? line.before : line.after) : ""}
      </td>
      <td class={`qd-mono qd-diff__code ${cls}`}>
        <Show when={line}>{(l) => <Text parts={parts} text={l().text} />}</Show>
      </td>
    </>
  );
}

/** A row of the two copies: a changed line beside its new text, what
 * changed inside both marked. */
const SplitRow: Component<{ row: Row }> = (props) => {
  const parts = () =>
    props.row.left?.kind === "del" && props.row.right?.kind === "add"
      ? inline(props.row.left.text, props.row.right.text)
      : undefined;
  return (
    <tr>
      {side(props.row.left, parts()?.[0], "del")}
      {side(props.row.right, parts()?.[1], "add")}
    </tr>
  );
};

const UnifiedRow: Component<{ line: Line }> = (props) => (
  <tr class={`qd-diff__${props.line.kind}`}>
    <td class="qd-diff__no">{props.line.before ?? ""}</td>
    <td class="qd-diff__no">{props.line.after ?? ""}</td>
    <td class="qd-mono qd-diff__code">
      {props.line.kind === "add" ? "+ " : props.line.kind === "del" ? "− " : "  "}
      {props.line.text}
    </td>
  </tr>
);

/**
 * A file's change, in hunks: as two copies side by side (before, after),
 * what changed inside a line marked, as git's review shows it; or as one
 * copy, taken out in red and put in in green.
 */
export const DiffView: Component<{ change: FileChange; mode: DiffMode }> = (props) => {
  const lines = () => lineDiff(props.change.before ?? "", props.change.after ?? "");
  const state = () =>
    props.change.before == null ? "added" : props.change.after == null ? "removed" : "changed";
  return (
    <section class="qd-diff">
      <header class="qd-diff__head">
        <span class="qd-mono">{props.change.file}</span>
        <span class="qd-hint">{t(`quality:drafts.history.${state()}`)}</span>
        <span class="qd-diff__added">+{counts(lines()).added}</span>
        <span class="qd-diff__removed">−{counts(lines()).removed}</span>
      </header>
      <table class={`qd-diff__lines qd-diff__lines--${props.mode}`}>
        <tbody>
          <For each={hunks(lines())}>
            {(hunk, k) => (
              <>
                <Show when={k() > 0}>
                  <tr class="qd-diff__gap">
                    <td colSpan={props.mode === "split" ? 4 : 3}>⋯</td>
                  </tr>
                </Show>
                <Show
                  when={props.mode === "split"}
                  fallback={<For each={hunk}>{(line) => <UnifiedRow line={line} />}</For>}
                >
                  <For each={sideBySide(hunk)}>{(row) => <SplitRow row={row} />}</For>
                </Show>
              </>
            )}
          </For>
        </tbody>
      </table>
    </section>
  );
};
