import { StateEffect, StateField, type EditorState } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";
import { paint, type Kind } from "./paint";

/** The dataset's fields changed (they come with each check). */
export const setFields = StateEffect.define<string[]>();

const fields = StateField.define<string[]>({
  create: () => [],
  update: (value, tr) => tr.effects.reduce((v, e) => (e.is(setFields) ? e.value : v), value),
});

const KINDS: Kind[] = ["field", "value", "expr", "keyword", "function", "text", "number", "param"];
const MARKS = Object.fromEntries(
  KINDS.map((k) => [k, Decoration.mark({ class: `qd-syntax-${k}` })]),
) as Record<Kind, Decoration>;

function build(state: EditorState): DecorationSet {
  const ranges = paint(state.doc.toString(), state.field(fields))
    .filter((p) => p.from < p.to)
    .map((p) => MARKS[p.kind].range(p.from, p.to));
  return Decoration.set(ranges, true);
}

const decorations = StateField.define<DecorationSet>({
  create: build,
  update: (set, tr) =>
    tr.docChanged || tr.effects.some((e) => e.is(setFields)) ? build(tr.state) : set,
  provide: (f) => EditorView.decorations.from(f),
});

/** Colours by meaning (`paint`), in the `--qd-syntax-*` of `quality.css`. */
const colours = EditorView.theme({
  // The YAML's own colour, drawn inside these, gives way to them.
  "[class*='qd-syntax-'] span:not([class*='qd-syntax-'])": { color: "inherit" },
  ".qd-syntax-expr": { color: "var(--color-text)" },
  // An expression's mark over a word's (`CASE` alone on its line) gives way.
  "[class*='qd-syntax-'] .qd-syntax-expr": { color: "inherit" },
  ".qd-syntax-field": { color: "var(--qd-syntax-field)" },
  ".qd-syntax-value": { color: "var(--qd-syntax-value)" },
  ".qd-syntax-keyword": { color: "var(--qd-syntax-special)", fontWeight: "600" },
  ".qd-syntax-param": { color: "var(--qd-syntax-special)", fontStyle: "italic" },
  ".qd-syntax-function": { color: "var(--qd-syntax-function)" },
  ".qd-syntax-number": { color: "var(--qd-syntax-function)" },
  ".qd-syntax-text": { color: "var(--qd-syntax-string)" },
});

/** A rules file coloured by what each word is: fields, values and SQL. */
export const painting = [fields, decorations, colours];
