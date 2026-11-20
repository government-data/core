import { search } from "@codemirror/search";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { t } from "./i18n";

/** The search panel's words, in the person's language. */
function phrases() {
  const w = (key: string) => t(`quality:editor.search.${key}`);
  return EditorState.phrases.of({
    Find: w("find"),
    Replace: w("replace"),
    next: w("next"),
    previous: w("previous"),
    all: w("all"),
    "match case": w("matchCase"),
    regexp: w("regexp"),
    "by word": w("byWord"),
    close: w("close"),
    replace: w("replaceOne"),
    "replace all": w("replaceAll"),
    "current match": w("currentMatch"),
    "on line": w("onLine"),
    "Go to line": w("goToLine"),
    go: w("go"),
  });
}

/** The search panel in the app's look: one height, aligned, its colours. */
const look = EditorView.theme({
  ".cm-panels": {
    background: "var(--color-surface)",
    color: "var(--color-text)",
  },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--color-border)" },
  // Find and its buttons, then replace and its own; the options in a
  // column at the right, one under another.
  ".cm-search": {
    display: "grid",
    gridTemplateColumns: "minmax(8rem, 14rem) repeat(3, max-content) 1fr",
    gridTemplateRows: "repeat(3, auto)",
    alignItems: "center",
    gap: "0.5rem 0.75rem",
    padding: "0.625rem 2.5rem 0.625rem 0.75rem",
    fontSize: "0.8125rem",
  },
  ".cm-search br": { display: "none" },
  ".cm-search [name=search]": { gridArea: "1 / 1" },
  ".cm-search [name=next]": { gridArea: "1 / 2" },
  ".cm-search [name=prev]": { gridArea: "1 / 3" },
  ".cm-search [name=select]": { gridArea: "1 / 4" },
  ".cm-search [name=replace]": { gridArea: "2 / 1" },
  ".cm-search button[name=replace]": { gridArea: "2 / 2" },
  ".cm-search [name=replaceAll]": { gridArea: "2 / 3 / 3 / 5", justifySelf: "start" },
  ".cm-search label:nth-of-type(1)": { gridArea: "1 / 5" },
  ".cm-search label:nth-of-type(2)": { gridArea: "2 / 5" },
  ".cm-search label:nth-of-type(3)": { gridArea: "3 / 5" },
  ".cm-search .cm-textfield": {
    height: "2rem",
    margin: "0",
    padding: "0 0.5rem",
    border: "1px solid var(--color-border)",
    borderRadius: "var(--radius-sm)",
    background: "var(--color-surface)",
    color: "var(--color-text)",
    fontSize: "0.8125rem",
  },
  ".cm-search .cm-textfield:focus": {
    outline: "2px solid var(--color-accent)",
    outlineOffset: "-1px",
  },
  ".cm-search .cm-button": {
    height: "2rem",
    margin: "0",
    padding: "0 0.75rem",
    border: "1px solid var(--color-border)",
    borderRadius: "var(--radius-sm)",
    backgroundImage: "none",
    background: "var(--color-surface)",
    color: "var(--color-text)",
    fontSize: "0.8125rem",
    cursor: "pointer",
  },
  ".cm-search .cm-button:hover": { background: "var(--color-muted-soft)" },
  ".cm-search label": {
    display: "inline-flex",
    alignItems: "center",
    gap: "0.375rem",
    margin: "0",
    color: "var(--color-muted)",
    whiteSpace: "nowrap",
    cursor: "pointer",
  },
  ".cm-search input[type=checkbox]": { margin: "0", accentColor: "var(--color-accent)" },
  ".cm-search button[name=close]": {
    top: "0.625rem",
    right: "0.75rem",
    fontSize: "1.125rem",
    color: "var(--color-muted)",
    cursor: "pointer",
  },
  ".cm-searchMatch": { background: "var(--color-accent-soft)" },
  ".cm-searchMatch-selected": { outline: "1px solid var(--color-accent)" },
});

/** Find and replace (Ctrl+F, Ctrl+H), above the text. */
export const searching = [search({ top: true }), phrases(), look];
