import { For, type Component } from "solid-js";
import { EditorView } from "@codemirror/view";

/** A bug for an error, a triangle for a warning (24×24, stroked). */
const PATHS = {
  error: [
    "m8 2 1.88 1.88",
    "M14.12 3.88 16 2",
    "M9 7.13v-1a3 3 0 1 1 6 0v1",
    "M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6",
    "M12 20v-9",
    "M6.53 9C4.6 8.8 3 7.1 3 5",
    "M6 13H2",
    "M3 21c0-2.1 1.7-3.9 3.8-4",
    "M20.97 5c0 2.1-1.6 3.8-3.5 4",
    "M22 13h-4",
    "M17.2 17c2.1.1 3.8 1.9 3.8 4",
  ],
  warning: [
    "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3",
    "M12 9v4",
    "M12 17h.01",
  ],
} as const;

export type Severity = keyof typeof PATHS;

/** A lint finding's icon, in its colour (`--status-critical`, `--status-warning`). */
export const LintIcon: Component<{ severity: Severity; label: string }> = (props) => (
  <svg
    class={`qd-lint-icon qd-lint-icon--${props.severity}`}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    role="img"
    aria-label={props.label}
  >
    <title>{props.label}</title>
    <For each={PATHS[props.severity]}>{(d) => <path d={d} />}</For>
  </svg>
);

/** The icon as a CSS mask, for the editor's gutter. */
function mask(severity: Severity) {
  const paths = PATHS[severity].map((d) => `<path d="${d}"/>`).join("");
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" ` +
    `stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">${paths}</svg>`;
  const url = `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
  return {
    content: "none",
    maskImage: url,
    WebkitMaskImage: url,
    maskRepeat: "no-repeat",
    WebkitMaskRepeat: "no-repeat",
    maskSize: "contain",
    WebkitMaskSize: "contain",
  };
}

/** The gutter's markers and the findings' tooltips with these icons and colours. */
export const lintLook = EditorView.theme({
  ".cm-lint-marker": { width: "0.9rem", height: "0.9rem" },
  ".cm-lint-marker-error": { ...mask("error"), backgroundColor: "var(--status-critical)" },
  ".cm-lint-marker-warning": { ...mask("warning"), backgroundColor: "var(--status-warning)" },
  ".cm-diagnostic-error": { borderLeftColor: "var(--status-critical)" },
  ".cm-diagnostic-warning": { borderLeftColor: "var(--status-warning)" },
});
