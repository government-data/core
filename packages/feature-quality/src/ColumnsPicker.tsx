import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { t } from "./i18n";
import { FilterIcon } from "./TestIcons";

/** What the picker needs: the dataset's fields, those chosen, and what to do with a choice. */
export interface Columns {
  fields: string[];
  chosen: string[];
  onChoose: (fields: string[]) => void;
  /** The button's text; a test's _Columns (n)_ if none. */
  label?: string;
  /** What the dialog says under the lists, and its button. */
  hint?: string;
  apply?: string;
}

/** One side of the picker: its fields, those selected on it, a click to select, a double click to move. */
const Side: Component<{
  title: string;
  fields: string[];
  selected: Set<string>;
  onSelect: (field: string, more: boolean) => void;
  onMove: (field: string) => void;
}> = (props) => (
  <div class="qd-picker__side">
    <h3 class="qd-section">
      {props.title} <span class="qd-hint">({props.fields.length})</span>
    </h3>
    <ul role="listbox" aria-multiselectable="true" aria-label={props.title}>
      <For each={props.fields}>
        {(f) => (
          <li
            role="option"
            aria-selected={props.selected.has(f)}
            class="qd-mono"
            onClick={(e) => props.onSelect(f, e.ctrlKey || e.metaKey)}
            onDblClick={() => props.onMove(f)}
          >
            {f}
          </li>
        )}
      </For>
    </ul>
  </div>
);

/** The buttons between the lists: each moves its fields across, if it has any. */
const Moves: Component<{
  moves: [string, string, string[]][];
  onMove: (kind: string, fields: string[]) => void;
}> = (props) => (
  <div class="qd-picker__moves">
    <For each={props.moves}>
      {([kind, mark, fields]) => (
        <button
          type="button"
          class="fk-btn fk-btn--sm"
          title={t(`quality:editor.test.${kind}`)}
          aria-label={t(`quality:editor.test.${kind}`)}
          disabled={fields.length === 0}
          onClick={() => props.onMove(kind, fields)}
        >
          {mark}
        </button>
      )}
    </For>
  </div>
);

/**
 * The fields to show in a test's records besides the key and those a rule
 * reads (decision 0043, point 3): two lists, excluded and included, a
 * search over both, and a field moved with a double click, the selected
 * ones with › and ‹, or all of them with » and «.
 */
const PickerDialog: Component<Columns & { onClose: () => void }> = (props) => {
  const [included, setIncluded] = createSignal<string[]>(
    props.chosen.filter((c) => props.fields.includes(c)),
  );
  const [selected, setSelected] = createSignal(new Set<string>());
  const [search, setSearch] = createSignal("");
  const shows = (f: string) => f.toLowerCase().includes(search().trim().toLowerCase());
  const excluded = createMemo(() => props.fields.filter((f) => !included().includes(f)));
  const select = (f: string, more: boolean) =>
    setSelected((s) => {
      const next = new Set(more ? s : []);
      if (s.has(f) && more) next.delete(f);
      else next.add(f);
      return next;
    });
  const include = (fs: string[]) => {
    setIncluded((i) => [...i, ...fs.filter((f) => !i.includes(f))]);
    setSelected(new Set<string>());
  };
  const exclude = (fs: string[]) => {
    setIncluded((i) => i.filter((f) => !fs.includes(f)));
    setSelected(new Set<string>());
  };
  const picked = (from: string[]) => from.filter((f) => selected().has(f));
  return (
    <Modal open size="wide" title={t("quality:editor.test.columnsTitle")} onClose={props.onClose}>
      <div class="qd-picker">
        <input
          class="qd-picker__search"
          type="search"
          placeholder={t("quality:editor.test.columnsSearch")}
          aria-label={t("quality:editor.test.columnsSearch")}
          value={search()}
          onInput={(e) => setSearch(e.currentTarget.value)}
        />
        <div class="qd-picker__lists">
          <Side
            title={t("quality:editor.test.excluded")}
            fields={excluded().filter(shows)}
            selected={selected()}
            onSelect={select}
            onMove={(f) => include([f])}
          />
          <Moves
            moves={[
              ["includeSelected", "›", picked(excluded())],
              ["includeAll", "»", excluded().filter(shows)],
              ["excludeSelected", "‹", picked(included())],
              ["excludeAll", "«", included().filter(shows)],
            ]}
            onMove={(kind, fs) => (kind.startsWith("include") ? include(fs) : exclude(fs))}
          />
          <Side
            title={t("quality:editor.test.included")}
            fields={included().filter(shows)}
            selected={selected()}
            onSelect={select}
            onMove={(f) => exclude([f])}
          />
        </div>
        <p class="qd-hint">{props.hint ?? t("quality:editor.test.columnsHint")}</p>
        <div class="qd-actions">
          <button type="button" class="fk-btn" onClick={props.onClose}>
            {t("quality:common.cancel")}
          </button>
          <button
            type="button"
            class="fk-btn fk-btn--primary"
            onClick={() => {
              props.onClose();
              props.onChoose(included());
            }}
          >
            {props.apply ?? t("quality:editor.test.columnsApply")}
          </button>
        </div>
      </div>
    </Modal>
  );
};

/** _Columns (n)_ beside a test's result: opens the picker; applied, the test runs again. */
export const ColumnsPicker: Component<Columns> = (props) => {
  const [open, setOpen] = createSignal(false);
  const label = () =>
    props.label ?? t("quality:editor.test.columns", { count: props.chosen.length });
  return (
    <>
      <button
        type="button"
        class="fk-btn qd-icon-btn"
        title={label()}
        aria-label={label()}
        onClick={() => setOpen(true)}
      >
        <FilterIcon />
        <span class="qd-icon-btn__count">{props.chosen.length}</span>
      </button>
      <Show when={open()}>
        <PickerDialog {...props} onClose={() => setOpen(false)} />
      </Show>
    </>
  );
};
