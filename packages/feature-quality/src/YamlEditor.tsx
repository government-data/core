import { createEffect, onCleanup, onMount, Show, type Component, type JSX } from "solid-js";
import {
  autocompletion,
  completionKeymap,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { yaml } from "@codemirror/lang-yaml";
import {
  bracketMatching,
  HighlightStyle,
  indentOnInput,
  syntaxHighlighting,
} from "@codemirror/language";
import { lintGutter, setDiagnostics } from "@codemirror/lint";
import { openSearchPanel, searchKeymap } from "@codemirror/search";
import { EditorState } from "@codemirror/state";
import { EditorView, highlightActiveLine, keymap, lineNumbers } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { context, suggest } from "./completion";
import { mountEditor } from "./mountEditor";
import { help } from "./help";
import { hoverHelp } from "./hover";
import { extending, setNotes, type ExtendsNote } from "./extending";
import { lintLook } from "./LintIcon";
import { searching } from "./searching";
import { t } from "./i18n";
import type { Mark } from "./editor";
import { painting, setFields } from "./painting";

/** YAML in the theme's colours (`--qd-syntax-*` in `quality.css`). */
const colours = HighlightStyle.define([
  { tag: tags.definition(tags.propertyName), color: "var(--qd-syntax-key)" },
  { tag: tags.string, color: "var(--qd-syntax-string)" },
  { tag: tags.special(tags.string), color: "var(--qd-syntax-special)" },
  { tag: tags.lineComment, color: "var(--qd-syntax-comment)", fontStyle: "italic" },
  {
    tag: [tags.separator, tags.punctuation, tags.squareBracket, tags.brace, tags.meta],
    color: "var(--qd-syntax-punctuation)",
  },
  { tag: [tags.labelName, tags.typeName, tags.keyword], color: "var(--qd-syntax-special)" },
]);

/** Completion from the YAML's schema and the dataset's fields. */
function completer(schema: () => Record<string, unknown> | undefined, fields: () => string[]) {
  return (cm: CompletionContext): CompletionResult | null => {
    const known = schema();
    const at = known && context(cm.state.doc.toString(), cm.pos);
    if (!known || !at || (at.at === "key" && !at.word && !cm.explicit)) return null;
    const options = suggest(known, at, fields()).map((s) => ({
      label: s.label,
      type: s.kind === "key" ? "property" : s.kind === "field" ? "variable" : "enum",
      info: help(at, s.label) ?? s.info,
      apply: s.kind === "key" ? `${s.label}: ` : s.label,
    }));
    return options.length ? { from: at.from, options, validFor: /^[\w-]*$/ } : null;
  };
}

const theme = EditorView.theme({
  "&": {
    height: "100%",
    fontSize: "0.8125rem",
    background: "var(--color-surface)",
    color: "var(--color-text)",
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
  ".qd-hover, .cm-completionInfo": { maxWidth: "24rem", padding: "0.375rem 0.5rem" },
  ".qd-extends-note": {
    padding: "0.125rem 0.5rem 0.25rem 2rem",
    color: "var(--color-muted)",
    fontStyle: "italic",
    whiteSpace: "pre-wrap",
  },
  ".cm-tooltip-autocomplete ul li[aria-selected]": {
    background: "var(--color-accent)",
    color: "var(--color-surface)",
  },
});

/** _Test_ (decision 0043): the rule at the cursor, or the whole file. */
export interface Testing {
  run: () => void;
  can: boolean;
  /** The rule at the cursor, if any. */
  rule?: string;
}

/** The editor's bar: _Tidy_ when offered, _Find_, and _Test_ when offered. */
const EditorTools: Component<{
  onTidy?: () => void;
  canTidy?: boolean;
  tidyNote?: string;
  onFind: () => void;
  testing?: Testing;
  /** More, before _Find_ (the console). */
  more?: JSX.Element;
}> = (props) => (
  <div class="qd-editor-tools">
    <Show when={props.tidyNote}>
      <span class="qd-hint" role="status">
        {props.tidyNote}
      </span>
    </Show>
    <Show when={props.onTidy}>
      {(tidy) => (
        <button
          type="button"
          class="fk-btn"
          disabled={!props.canTidy}
          title={t("quality:editor.tidyHint")}
          onClick={() => tidy()()}
        >
          {t("quality:editor.tidy")}
        </button>
      )}
    </Show>
    {props.more}
    <button
      type="button"
      class="fk-btn"
      title={t("quality:editor.search.keys")}
      onClick={() => props.onFind()}
    >
      {t("quality:editor.search.open")}
    </button>
    <Show when={props.testing}>
      {(test) => (
        <button
          type="button"
          class="fk-btn fk-btn--primary"
          disabled={!test().can}
          title={`${
            test().rule
              ? t("quality:editor.test.runRule", { rule: test().rule })
              : t("quality:editor.test.runFile")
          }. ${t("quality:editor.test.hint")}`}
          onClick={() => test().run()}
        >
          {t("quality:editor.test.run")}
        </button>
      )}
    </Show>
  </div>
);

/**
 * A YAML editor (CodeMirror 6): line numbers, undo, the marks of its last
 * check, the line to show, and completion from `schema` with the dataset's
 * `fields` (Ctrl+Space, or as a word is typed). `onChange` gets the text.
 */
export const YamlEditor: Component<{
  text: string;
  marks: Mark[];
  line?: number;
  schema?: Record<string, unknown>;
  fields: string[];
  /** What each `extends` does, at its line (decision 0045). */
  notes?: ExtendsNote[];
  /** _Tidy_ (decision 0041), enabled while `canTidy`. */
  onTidy?: () => void;
  canTidy?: boolean;
  /** Why _Tidy_ did nothing, if it did not. */
  tidyNote?: string;
  testing?: Testing;
  /** More in its bar, before _Find_. */
  tools?: JSX.Element;
  onChange: (text: string) => void;
  /** The line (from 1) the cursor is on, as it moves. */
  onCursor?: (line: number) => void;
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
          indentOnInput(),
          bracketMatching(),
          lintGutter(),
          lintLook,
          yaml(),
          syntaxHighlighting(colours),
          painting,
          hoverHelp,
          extending,
          searching,
          autocompletion({
            icons: false,
            override: [
              completer(
                () => props.schema,
                () => props.fields,
              ),
            ],
          }),
          theme,
          EditorView.lineWrapping,
          keymap.of([
            { key: "Mod-h", run: openSearchPanel, preventDefault: true },
            ...searchKeymap,
            ...completionKeymap,
            indentWithTab,
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) props.onChange(u.state.doc.toString());
            if (u.docChanged || u.selectionSet) {
              props.onCursor?.(u.state.doc.lineAt(u.state.selection.main.head).number);
            }
          }),
        ],
      }),
    );
  });
  onCleanup(() => view?.destroy());

  // A text from outside (restored): replace the document.
  createEffect(() => {
    const text = props.text;
    if (view && view.state.doc.toString() !== text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
    }
  });

  createEffect(() => {
    const fields = props.fields;
    view?.dispatch({ effects: setFields.of(fields) });
  });

  createEffect(() => {
    const notes = props.notes ?? [];
    view?.dispatch({ effects: setNotes.of(notes) });
  });

  createEffect(() => {
    const marks = props.marks;
    if (!view) return;
    const length = view.state.doc.length;
    const clamped = marks.map((m) => ({
      ...m,
      from: Math.min(m.from, length),
      to: Math.min(m.to, length),
    }));
    view.dispatch(setDiagnostics(view.state, clamped));
  });

  createEffect(() => {
    const line = props.line;
    if (!view || !line || line > view.state.doc.lines) return;
    const at = view.state.doc.line(line).from;
    view.dispatch({
      selection: { anchor: at },
      effects: EditorView.scrollIntoView(at, { y: "start" }),
    });
  });

  return (
    <>
      <EditorTools
        onTidy={props.onTidy}
        canTidy={props.canTidy}
        tidyNote={props.tidyNote}
        onFind={() => view && openSearchPanel(view)}
        testing={props.testing}
        more={props.tools}
      />
      <div class="qd-editor" ref={parent} />
    </>
  );
};
