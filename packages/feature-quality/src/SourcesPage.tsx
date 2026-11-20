import "./quality.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { ConfirmDialog, Status } from "@knime-orchestador/feature-kit/ui";
import { may, me, sourcesApi, type SourceView } from "./api";
import { t } from "./i18n";
import { LoadFailed, notify, warn } from "./Notify";
import { MapDialog } from "./MapDialog";
import { deleteRefusal } from "./sources";
import { SourcesNavIcon } from "./NavIcon";
import { SourceForm } from "./SourceForm";
import { TablesDialog } from "./TablesDialog";

/**
 * Data → Sources (decision 0036): a tenant's sources, their credentials by
 * name only; create, edit, delete (refused while a dataset reads one), see
 * its tables, read when it is saved (0042), and map one of them (0038).
 */
export const SourcesPage: Component = () => {
  const [list, { refetch }] = createResource(sourcesApi.list);
  const [drivers] = createResource(sourcesApi.drivers);
  const [who] = createResource(me);
  const [form, setForm] = createSignal<{ editing?: SourceView }>();
  const [deleting, setDeleting] = createSignal<SourceView>();
  const [browsing, setBrowsing] = createSignal<string>();
  const [mapping, setMapping] = createSignal<{ source: string; table: string }>();
  const manages = () => may(who(), "quality:ManageSources");

  const saved = (text: string, name: string) => {
    setForm(undefined);
    notify(text);
    void refetch();
    setBrowsing(name);
  };

  const pick = (table: string) => {
    const source = browsing();
    setBrowsing(undefined);
    if (source) setMapping({ source, table });
  };

  const remove = async () => {
    const source = deleting();
    if (!source) return;
    try {
      await sourcesApi.remove(source.name);
      notify(t("quality:sources.deleted", { name: source.name }));
      void refetch();
    } catch (err) {
      warn(deleteRefusal(err));
    } finally {
      setDeleting(undefined);
    }
  };

  return (
    <section class="feature qd-page">
      <header class="qd-head">
        <div>
          <h1>{t("quality:sources.title")}</h1>
          <p class="qd-hint">{t("quality:sources.subtitle")}</p>
        </div>
        <Show when={manages() && drivers()}>
          <button class="fk-btn fk-btn--primary" onClick={() => setForm({})}>
            {t("quality:sources.new")}
          </button>
        </Show>
      </header>
      <LoadFailed when={list.error} />
      <Show when={list()}>
        {(sources) => (
          <SourcesTable
            sources={sources()}
            manages={manages()}
            onEdit={(s) => setForm({ editing: s })}
            onDelete={setDeleting}
            onTables={(s) => setBrowsing(s.name)}
          />
        )}
      </Show>
      <Show when={form() && drivers()}>
        <SourceForm
          drivers={drivers() ?? []}
          editing={form()?.editing}
          onClose={() => setForm(undefined)}
          onSaved={saved}
        />
      </Show>
      <Show when={browsing()}>
        {(name) => (
          <TablesDialog
            source={name()}
            manages={manages()}
            onPick={pick}
            onClose={() => setBrowsing(undefined)}
          />
        )}
      </Show>
      <Show when={mapping()}>
        {(m) => (
          <MapDialog source={m().source} table={m().table} onClose={() => setMapping(undefined)} />
        )}
      </Show>
      <ConfirmDialog
        open={!!deleting()}
        title={t("quality:sources.deleteTitle", { name: deleting()?.name ?? "" })}
        message={t("quality:sources.deleteBody")}
        onCancel={() => setDeleting(undefined)}
        onConfirm={() => void remove()}
      />
    </section>
  );
};

const SourcesTable: Component<{
  sources: SourceView[];
  manages: boolean;
  onEdit: (s: SourceView) => void;
  onDelete: (s: SourceView) => void;
  onTables: (s: SourceView) => void;
}> = (props) => (
  <div class="card">
    <Show
      when={props.sources.length > 0}
      fallback={<p class="qd-hint">{t("quality:sources.empty")}</p>}
    >
      <table class="table qd-table">
        <thead>
          <tr>
            <th>{t("quality:sources.col.name")}</th>
            <th>{t("quality:sources.col.driver")}</th>
            <th>{t("quality:sources.col.host")}</th>
            <th>{t("quality:sources.col.secrets")}</th>
            <th>{t("quality:sources.col.rotated")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          <For each={props.sources}>
            {(s) => (
              <tr>
                <td class="qd-mono">{s.name}</td>
                <td>
                  <span class="qd-driver">
                    <SourcesNavIcon size={14} />
                    {s.driver}
                  </span>
                </td>
                <td class="qd-mono">{[s.fields.host, s.fields.port].filter(Boolean).join(":")}</td>
                <td>
                  <Status
                    tone="good"
                    label={t("quality:sources.secretsSet", { count: s.secrets.length })}
                  />
                </td>
                <td>
                  {s.previous
                    ? t("quality:sources.previousKept")
                    : t("quality:sources.previousNone")}
                </td>
                <td class="qd-row-actions">
                  <button class="fk-btn fk-btn--sm" onClick={() => props.onTables(s)}>
                    {t("quality:sources.tables.action")}
                  </button>
                  <Show when={props.manages}>
                    <button class="fk-btn fk-btn--sm" onClick={() => props.onEdit(s)}>
                      {t("quality:common.edit")}
                    </button>
                    <button
                      class="fk-btn fk-btn--sm fk-btn--danger"
                      onClick={() => props.onDelete(s)}
                    >
                      {t("quality:common.delete")}
                    </button>
                  </Show>
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </Show>
  </div>
);
