import { createResource, For, Show, type Component } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { drafted, draftsOf } from "./explorerCache";
import { t } from "./i18n";
import { keyOf, openFiles, pathOf, type OpenFile } from "./openFiles";

/**
 * The work area's tabs (decision 0043): one a file open, the one shown
 * marked, `●` on one with something unsaved; closing the one shown goes
 * to its neighbour, or, when none is left, to _Nothing open_.
 */
export const FileTabs: Component<{ current: OpenFile }> = (props) => {
  const navigate = useNavigate();
  // The tab shown: away from it first, so the editor leaving does not open
  // it again; then closed.
  const close = (f: OpenFile) => {
    if (keyOf(f) !== keyOf(props.current)) return void openFiles.close(f);
    const rest = openFiles.tabs().filter((o) => keyOf(o) !== keyOf(f));
    const at = openFiles.tabs().findIndex((o) => keyOf(o) === keyOf(f));
    const next = rest[at] ?? rest[at - 1];
    navigate(next ? pathOf(next) : "/quality/rules");
    openFiles.close(f);
  };
  return (
    <nav class="qd-file-tabs" aria-label={t("quality:editor.files.label")}>
      <For each={openFiles.tabs()}>
        {(f) => (
          <span
            class="qd-file-tab"
            classList={{ "qd-file-tab--current": keyOf(f) === keyOf(props.current) }}
          >
            <A href={pathOf(f)} title={keyOf(f)}>
              <span class="qd-hint">{f.dataset}/</span>
              <span class="qd-mono">{f.file}</span>
              <Show when={f.draft}>
                <small class="qd-pill">{t("quality:editor.files.draft")}</small>
              </Show>
              <Show when={openFiles.isUnsaved(f)}>
                <span class="qd-file-tab__unsaved" title={t("quality:editor.changed")}>
                  {" "}
                  ●
                </span>
              </Show>
            </A>
            <button
              type="button"
              class="qd-file-tab__close"
              aria-label={t("quality:editor.files.close", { file: f.file })}
              onClick={() => close(f)}
            >
              ×
            </button>
          </span>
        )}
      </For>
    </nav>
  );
};

/**
 * Which version of the file the editor shows (decisions 0043 and 0050):
 * `main`, or one of its column's drafts, newest first; choosing one opens the file
 * as it has it, and saving goes on it.
 */
export const DraftPicker: Component<{
  dataset: string;
  /** The file open: only its column's drafts are offered (decision 0050). */
  file: string;
  draft?: string;
  onPick: (draft: string | undefined) => void;
}> = (props) => {
  const [drafts] = createResource(
    () => ({ dataset: props.dataset, draft: props.draft, n: drafted() }),
    async ({ dataset }) => (await draftsOf(dataset)).filter((d) => d.file === props.file),
  );
  const ids = () => {
    const known = (drafts() ?? []).map((d) => d.id);
    return props.draft && !known.includes(props.draft) ? [props.draft, ...known] : known;
  };
  return (
    <label class="qd-field qd-inline">
      <span>{t("quality:editor.files.version")}</span>
      <select
        value={props.draft ?? ""}
        onChange={(e) => props.onPick(e.currentTarget.value || undefined)}
      >
        <option value="">main</option>
        <For each={ids()}>
          {(id) => <option value={id}>{t("quality:editor.files.draftId", { id })}</option>}
        </For>
      </select>
    </label>
  );
};
