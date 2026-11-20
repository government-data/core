import { createSignal, Show, type Component } from "solid-js";
import { Modal, toast } from "@knime-orchestador/feature-kit/ui";
import { QualityError, sourcesApi } from "./api";
import { t } from "./i18n";
import { watchMap } from "./mapWatch";
import { validName } from "./sources";

/** One to three parts, as a mapping's `from.table` takes them. */
const TABLE = /^[A-Za-z0-9_$#]{1,128}(\.[A-Za-z0-9_$#]{1,128}){0,2}$/;

/**
 * Asking for a map of a source's table (decisions 0038 and 0053), picked from its
 * listing (0042) or typed: a sample is profiled, the dataset is published
 * with the mapping and rules it suggests, and its first cut collected; a toast says when it is ready.
 */
export const MapDialog: Component<{ source: string; table?: string; onClose: () => void }> = (
  props,
) => {
  const [table, setTable] = createSignal(props.table ?? "");
  const [dataset, setDataset] = createSignal("");
  const [refused, setRefused] = createSignal<string>();
  const ready = () => TABLE.test(table()) && validName(dataset());

  const ask = async (e: Event) => {
    e.preventDefault();
    setRefused(undefined);
    try {
      const out = await sourcesApi.map(props.source, table(), dataset());
      toast("info", t("quality:sources.map.queued", { dataset: out.dataset }), undefined, null);
      watchMap(out.job, out.dataset, props.source, table());
      props.onClose();
    } catch (err) {
      const exists = err instanceof QualityError && err.status === 409;
      setRefused(t(exists ? "quality:sources.map.exists" : "quality:sources.map.refused"));
    }
  };

  return (
    <Modal
      open
      title={t("quality:sources.map.title", { name: props.source })}
      onClose={props.onClose}
    >
      <form class="qd-form" onSubmit={(e) => void ask(e)}>
        <p class="qd-hint">{t("quality:sources.map.body")}</p>
        <label class="qd-field">
          <span>{t("quality:sources.map.table")} *</span>
          <input
            class="qd-mono"
            autocomplete="off"
            value={table()}
            onInput={(e) => setTable(e.currentTarget.value.trim())}
          />
          <small>{t("quality:sources.map.tableHint")}</small>
        </label>
        <label class="qd-field">
          <span>{t("quality:sources.map.dataset")} *</span>
          <input
            class="qd-mono"
            autocomplete="off"
            value={dataset()}
            onInput={(e) => setDataset(e.currentTarget.value.trim())}
          />
          <small>{t("quality:sources.map.datasetHint")}</small>
        </label>
        <Show when={refused()}>{(text) => <p class="qd-error">{text()}</p>}</Show>
        <div class="qd-actions">
          <button type="button" class="fk-btn" onClick={props.onClose}>
            {t("quality:common.cancel")}
          </button>
          <button type="submit" class="fk-btn fk-btn--primary" disabled={!ready()}>
            {t("quality:sources.map.submit")}
          </button>
        </div>
      </form>
    </Modal>
  );
};
