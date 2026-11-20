import { createEffect, onCleanup, onMount, type Component } from "solid-js";
import { autocompletion, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { PostgreSQL, sql } from "@codemirror/lang-sql";
import { bracketMatching, HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView, highlightActiveLine, keymap, lineNumbers } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { mountEditor } from "./mountEditor";

const colours = HighlightStyle.define([
  { tag: tags.keyword, color: "var(--qd-syntax-key)", fontWeight: "600" },
  { tag: tags.string, color: "var(--qd-syntax-string)" },
  // A quoted name (`"loans_bronze"`) is a name, not text.
  { tag: tags.special(tags.string), color: "var(--color-text)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--qd-syntax-special)" },
  {
    tag: [tags.lineComment, tags.blockComment],
    color: "var(--qd-syntax-comment)",
    fontStyle: "italic",
  },
  { tag: [tags.punctuation, tags.operator, tags.paren], color: "var(--qd-syntax-punctuation)" },
  { tag: tags.special(tags.name), color: "var(--qd-syntax-special)" },
  // As the rules' SQL paints them: types and functions.
  { tag: tags.typeName, color: "var(--qd-syntax-value)" },
  {
    tag: [tags.function(tags.variableName), tags.standard(tags.name)],
    color: "var(--qd-syntax-function)",
  },
]);

const theme = EditorView.theme({
  "&": {
    minHeight: "9rem",
    maxHeight: "18rem",
    fontSize: "0.8125rem",
    background: "var(--color-surface)",
    color: "var(--color-text)",
    border: "1px solid var(--color-border)",
    borderRadius: "var(--radius-sm)",
  },
  ".cm-scroller": { fontFamily: "var(--font-mono)" },
  ".cm-gutters": {
    background: "var(--color-surface)",
    borderRight: "1px solid var(--color-border)",
  },
  "&.cm-focused": { outline: "2px solid var(--color-accent)" },
  ".cm-tooltip": {
    background: "var(--color-surface)",
    border: "1px solid var(--color-border)",
    color: "var(--color-text)",
  },
});

/**
 * The console's statement (decision 0054): SQL coloured, the lake's
 * tables and their columns completed (`lake.<namespace>.<table>`),
 * Ctrl+Enter to run.
 */
export const SqlEditor: Component<{
  text: string;
  /** The lake's tables, by `lake.<namespace>.<table>`, with their columns. */
  tables: Record<string, string[]>;
  label: string;
  onChange: (text: string) => void;
  onRun: () => void;
}> = (props) => {
  let parent!: HTMLDivElement;
  let view: EditorView | undefined;
  onMount(() => {
    view = mountEditor(
      parent,
      EditorState.create({
        doc: props.text,
        extensions: [
          lineNumbers(),
          history(),
          highlightActiveLine(),
          bracketMatching(),
          sql({ dialect: PostgreSQL, schema: props.tables, upperCaseKeywords: true }),
          syntaxHighlighting(colours),
          autocompletion({ icons: false }),
          theme,
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({ "aria-label": props.label }),
          keymap.of([
            { key: "Mod-Enter", run: () => (props.onRun(), true), preventDefault: true },
            ...completionKeymap,
            indentWithTab,
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) props.onChange(u.state.doc.toString());
          }),
        ],
      }),
    );
    view.focus();
  });
  onCleanup(() => view?.destroy());

  // A text from outside (formatted): replace the document.
  createEffect(() => {
    const text = props.text;
    if (view && view.state.doc.toString() !== text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
    }
  });

  return <div class="qd-console__sql" ref={parent} />;
};
