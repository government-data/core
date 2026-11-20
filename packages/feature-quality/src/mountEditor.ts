import type { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

/**
 * An editor (CodeMirror) mounted in an open shadow root on `host`. There
 * CodeMirror keeps its styles in constructed style sheets, which the app's
 * CSP (`style-src 'self'`, no 'unsafe-inline') allows; on the document it
 * adds `<style>` elements the CSP refuses, and the editor shows unstyled.
 * The theme's variables (`--color-*`, `--qd-syntax-*` on the host) are
 * inherited across it.
 */
export function mountEditor(host: HTMLElement, state: EditorState): EditorView {
  const root = host.shadowRoot ?? host.attachShadow({ mode: "open" });
  return new EditorView({ root, parent: root, state });
}
