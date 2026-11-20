import { StateEffect, StateField, type EditorState, type Range } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  WidgetType,
  hoverTooltip,
  type DecorationSet,
} from "@codemirror/view";
import type { RuleView } from "./api";
import { t } from "./i18n";

/** What a rule that `extends` a named one does, said at the line that cites it. */
export interface ExtendsNote {
  line: number;
  text: string;
}

/**
 * What each `extends` does (decision 0045), from the rules as read: the
 * named rule and, as it results, when the rule applies and when it fails;
 * a list of fields gives one rule each, said once.
 */
export function extendsNotes(rules: RuleView[]): ExtendsNote[] {
  const byLine = new Map<number, string[]>();
  for (const r of rules) {
    if (!r.extends) continue;
    const text = t("quality:editor.extends", {
      named: r.extends.named,
      applies: r.applies,
      fails: r.fails,
    });
    const said = byLine.get(r.extends.line) ?? [];
    if (!said.includes(text)) said.push(text);
    byLine.set(r.extends.line, said);
  }
  return [...byLine].map(([line, texts]) => ({ line, text: texts.join("\n") }));
}

/** The notes changed (they come with each check). */
export const setNotes = StateEffect.define<ExtendsNote[]>();

const notes = StateField.define<ExtendsNote[]>({
  create: () => [],
  update: (value, tr) => tr.effects.reduce((v, e) => (e.is(setNotes) ? e.value : v), value),
});

class Note extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  override eq(other: Note) {
    return other.text === this.text;
  }
  toDOM() {
    const dom = document.createElement("div");
    dom.className = "qd-extends-note";
    dom.textContent = this.text;
    return dom;
  }
}

function build(state: EditorState): DecorationSet {
  const ranges: Range<Decoration>[] = state
    .field(notes)
    .filter((n) => n.line >= 1 && n.line <= state.doc.lines)
    .map((n) =>
      Decoration.widget({ widget: new Note(n.text), block: true, side: 1 }).range(
        state.doc.line(n.line).to,
      ),
    );
  return Decoration.set(ranges, true);
}

const shown = StateField.define<DecorationSet>({
  create: build,
  update: (set, tr) =>
    tr.docChanged || tr.effects.some((e) => e.is(setNotes)) ? build(tr.state) : set,
  provide: (f) => EditorView.decorations.from(f),
});

/** The same note with the pointer over its `extends` line. */
const hover = hoverTooltip((view, pos) => {
  const line = view.state.doc.lineAt(pos);
  const note = view.state.field(notes).find((n) => n.line === line.number);
  if (!note || !/^\s*extends:/.test(line.text)) return null;
  return {
    pos: line.from,
    end: line.to,
    above: true,
    create: () => {
      const dom = document.createElement("div");
      dom.className = "qd-hover qd-extends-note";
      dom.textContent = note.text;
      return { dom };
    },
  };
});

/** Under each `extends`, and on hover over it, what the rule does. */
export const extending = [notes, shown, hover];
