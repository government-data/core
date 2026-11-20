import { createResource, createSignal, Show, type Component } from "solid-js";
import { ConfirmDialog } from "@knime-orchestador/feature-kit/ui";
import { sourcesApi, type SourceView } from "./api";
import { t } from "./i18n";
import { warn } from "./Notify";
import { SourceForm } from "./SourceForm";
import { deleteRefusal } from "./sources";

/** The drivers a source's form offers, asked once. */
let known: ReturnType<typeof sourcesApi.drivers> | undefined;
const loadDrivers = () => {
  if (!known) {
    known = sourcesApi.drivers();
    known.catch(() => (known = undefined));
  }
  return known;
};

/** The tree's header (decision 0043): its sources, and _+_ for a new one. */
export const NewSource: Component<{ manages: boolean; onSaved: (name: string) => void }> = (
  props,
) => {
  const [open, setOpen] = createSignal(false);
  const [drivers] = createResource(loadDrivers);
  return (
    <div class="qd-explorer__head">
      <span>{t("quality:sources.title")}</span>
      <Show when={props.manages && drivers()}>
        <button
          type="button"
          class="qd-explorer__add"
          title={t("quality:sources.new")}
          aria-label={t("quality:sources.new")}
          onClick={() => setOpen(true)}
        >
          +
        </button>
      </Show>
      <Show when={open() && drivers()}>
        <SourceForm
          drivers={drivers() ?? []}
          onClose={() => setOpen(false)}
          onSaved={(_text, name) => {
            setOpen(false);
            props.onSaved(name);
          }}
        />
      </Show>
    </div>
  );
};

/** On a source of the tree: _Edit_ its form, _Delete_ it (refused while a dataset reads it). */
export const SourceActions: Component<{ source: SourceView; onChanged: () => void }> = (props) => {
  const [drivers] = createResource(loadDrivers);
  const [editing, setEditing] = createSignal(false);
  const [deleting, setDeleting] = createSignal(false);
  const remove = async () => {
    setDeleting(false);
    try {
      await sourcesApi.remove(props.source.name);
      props.onChanged();
    } catch (err) {
      warn(deleteRefusal(err));
    }
  };
  return (
    <>
      <button type="button" class="fk-btn fk-btn--sm" onClick={() => setEditing(true)}>
        {t("quality:common.edit")}
      </button>
      <button type="button" class="fk-btn fk-btn--sm" onClick={() => setDeleting(true)}>
        {t("quality:common.delete")}
      </button>
      <Show when={editing() && drivers()}>
        <SourceForm
          drivers={drivers() ?? []}
          editing={props.source}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false);
            props.onChanged();
          }}
        />
      </Show>
      <ConfirmDialog
        open={deleting()}
        title={t("quality:sources.deleteTitle", { name: props.source.name })}
        message={t("quality:sources.deleteBody")}
        onCancel={() => setDeleting(false)}
        onConfirm={() => void remove()}
      />
    </>
  );
};
