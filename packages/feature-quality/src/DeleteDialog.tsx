import { createSignal, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { deleteApi, may, QualityError } from "./api";
import { knownMe, listingsChanged, mappingsChanged } from "./explorerCache";
import { t } from "./i18n";
import { notify, warn } from "./Notify";
import type { MenuItem } from "./TreeMenu";

/**
 * What is to be deleted: a dataset, a lake table no dataset reads, or a
 * lake namespace with its `tables`.
 */
export type Deleting =
  | { dataset: string }
  | { namespace: string; table: string }
  | { namespace: string; tables: number };

/** Its name, as it is typed to confirm: the dataset's, `namespace.table`, or the namespace's. */
const nameOf = (d: Deleting) =>
  "dataset" in d ? d.dataset : "table" in d ? `${d.namespace}.${d.table}` : d.namespace;

/** What the dialog says goes. */
const whatGoes = (d: Deleting) =>
  "dataset" in d
    ? t("quality:delete.dataset")
    : "table" in d
      ? t("quality:delete.table")
      : t("quality:delete.namespace", { count: d.tables });

const [deleting, setDeleting] = createSignal<Deleting>();

/** _Delete_ in a row's menu, for whoever may manage datasets (decision 0055). */
export function deleteMenu(what: Deleting | undefined): MenuItem[] {
  if (!what || !may(knownMe(), "quality:ManageDatasets")) return [];
  return [{ label: t("quality:delete.action"), run: () => setDeleting(what) }];
}

/** The refusal's text key, by its status and word. */
function refusal(err: unknown): string {
  const body =
    err instanceof QualityError ? (err.body as { error?: string } | undefined) : undefined;
  const known = ["running", "in_use", "confirm", "forbidden", "not_found"];
  return body?.error && known.includes(body.error)
    ? `quality:delete.refused.${body.error}`
    : "quality:delete.refused.other";
}

/**
 * Deleting a dataset or a lake table (decision 0055): what goes said, its
 * name typed again; _Delete_ only when it matches.
 */
const DeleteDialog: Component<{ what: Deleting; onClose: () => void }> = (props) => {
  const [typed, setTyped] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const name = () => nameOf(props.what);
  const run = async () => {
    setBusy(true);
    try {
      const w = props.what;
      if ("dataset" in w) await deleteApi.dataset(w.dataset, typed());
      else if ("table" in w) await deleteApi.lakeTable(w.namespace, w.table, typed());
      else await deleteApi.lakeNamespace(w.namespace, typed());
      notify(t("quality:delete.done", { name: name() }));
      mappingsChanged();
      listingsChanged();
      props.onClose();
    } catch (err) {
      warn(t(refusal(err), { name: name() }));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open title={t("quality:delete.title")} onClose={props.onClose}>
      <form
        class="qd-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (typed() === name()) void run();
        }}
      >
        <p>
          <code class="qd-delete-name">{name()}</code>
        </p>
        <p>{whatGoes(props.what)}</p>
        <p class="qd-error">{t("quality:delete.undone")}</p>
        <label class="qd-field">
          <span>
            {t("quality:delete.type")} <code class="qd-delete-name">{name()}</code>
          </span>
          <input
            class="qd-mono"
            autocomplete="off"
            spellcheck={false}
            value={typed()}
            onInput={(e) => setTyped(e.currentTarget.value)}
          />
        </label>
        <div class="qd-actions">
          <button type="button" class="fk-btn" onClick={props.onClose}>
            {t("quality:common.cancel")}
          </button>
          <button
            type="submit"
            class="fk-btn fk-btn--danger"
            disabled={busy() || typed() !== name()}
          >
            {t("quality:delete.confirm")}
          </button>
        </div>
      </form>
    </Modal>
  );
};

/** The dialog opened by a row's menu, once, outside the tree. */
export const DeleteHost: Component = () => (
  <Show when={deleting()} keyed>
    {(what) => <DeleteDialog what={what} onClose={() => setDeleting(undefined)} />}
  </Show>
);
