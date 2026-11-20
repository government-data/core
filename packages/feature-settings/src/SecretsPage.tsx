/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { ConfirmDialog } from "@knime-orchestador/feature-kit/ui";
import { formatDateTime } from "@knime-orchestador/i18n";
import { SettingsApiError, secretsApi, type GlobalSecret } from "./api";
import { Field, NoticeBox, SecretInput, SectionHeader, useAction } from "./common";
import { ConnectionForm } from "./ConnectionForm";
import { connectionValue, engineById, missingFields } from "./connections";
import { t } from "./i18n";

/** Like a flow variable's name. */
const NAME = /^[A-Za-z_][A-Za-z0-9_.-]{0,99}$/;

/**
 * Settings → Global variables. A secret (a database connection, an API
 * key…) is registered once here and any flow picks it in its variables; its
 * value is stored encrypted and never shown, and when it changes every flow
 * that uses it runs with the new one from its next execution.
 */
export const SecretsPage: Component = () => {
  const [list, { refetch }] = createResource(secretsApi.list);
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [value, setValue] = createSignal("");
  // The secret a new value is being written for.
  const [changing, setChanging] = createSignal<string | null>(null);
  const [newValue, setNewValue] = createSignal("");
  const [newDescription, setNewDescription] = createSignal("");
  const [deleting, setDeleting] = createSignal<GlobalSecret>();
  const create = useAction();
  const row = useAction();
  // What is created: a single value or a database connection.
  const [kind, setKind] = createSignal<"secret" | "database">("secret");
  // The connection being created: its engine and its fields.
  const [engine, setEngine] = createSignal("");
  const [connFields, setConnFields] = createSignal<Record<string, string>>({});
  // Editing a connection: its fields (the public ones, loaded; the secret ones, empty).
  const [editFields, setEditFields] = createSignal<Record<string, string>>({});
  const connMissing = () => {
    const e = engineById(engine());
    return !e || missingFields(e, connFields()).length > 0;
  };

  const validName = () => NAME.test(name().trim());

  const submit = (e: Event) => {
    e.preventDefault();
    void create.run(async () => {
      const saved = await secretsApi.create({
        name: name().trim(),
        description: description().trim(),
        value: kind() === "database" ? connectionValue(engine(), connFields()) : value(),
      });
      setName("");
      setDescription("");
      setValue("");
      setConnFields({});
      await refetch();
      return t("settings:secrets.created", { name: saved.name });
    });
  };

  const edit = (secret: GlobalSecret) => {
    setNewValue("");
    setEditFields({ ...(secret.meta.fields ?? {}) });
    setNewDescription(secret.description);
    setChanging(secret.id);
  };

  const saveEdit = (secret: GlobalSecret) =>
    void row.run(async () => {
      await secretsApi.update(secret.id, {
        description: newDescription().trim(),
        // A connection goes whole (its empty secrets keep the saved ones); an
        // empty value keeps the saved one.
        ...(secret.kind === "database"
          ? { value: connectionValue(secret.meta.engine ?? "", editFields()) }
          : newValue()
            ? { value: newValue() }
            : {}),
      });
      setChanging(null);
      setNewValue("");
      await refetch();
      return t("settings:secrets.updated", { name: secret.name });
    });

  const remove = () => {
    const secret = deleting();
    if (!secret) return;
    void row
      .run(async () => {
        await secretsApi.remove(secret.id);
        await refetch();
        return t("settings:secrets.deleted", { name: secret.name });
      })
      .finally(() => setDeleting(undefined));
  };

  return (
    <section class="feature">
      <SectionHeader
        title={t("settings:secrets.title")}
        subtitle={t("settings:secrets.subtitle")}
      />
      <Show when={list.error}>
        <p class="st-notice st-notice--error">
          {list.error instanceof SettingsApiError
            ? list.error.message
            : t("settings:secrets.loadError")}
        </p>
      </Show>

      <form class="card st-form" onSubmit={submit}>
        <h2 class="st-subtitle">{t("settings:secrets.new")}</h2>
        <Field label={t("settings:secrets.kind")}>
          <select
            value={kind()}
            onChange={(e) => setKind(e.currentTarget.value as "secret" | "database")}
          >
            <option value="secret">{t("settings:secrets.kindSecret")}</option>
            <option value="database">{t("settings:secrets.kindDatabase")}</option>
          </select>
        </Field>
        <div class="st-row">
          <Field
            label={t("settings:secrets.name")}
            error={
              create.fields().name ??
              (name().trim() && !validName() ? t("settings:secrets.invalidName") : undefined)
            }
            hint={t("settings:secrets.nameHint")}
          >
            <input
              class="st-mono"
              required
              autocomplete="off"
              placeholder={t("settings:secrets.namePlaceholder")}
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
            />
          </Field>
          <Show when={kind() === "secret"}>
            <Field label={t("settings:secrets.value")} error={create.fields().value}>
              <SecretInput required value={value()} onInput={setValue} />
            </Field>
          </Show>
        </div>
        <Show when={kind() === "database"}>
          <ConnectionForm
            engine={engine()}
            onEngine={(e) => {
              setEngine(e);
              setConnFields({});
            }}
            fields={connFields()}
            onField={(k, v) => setConnFields({ ...connFields(), [k]: v })}
            errors={create.fields()}
          />
        </Show>
        <Field label={t("settings:secrets.description")} error={create.fields().description}>
          <input
            placeholder={t("settings:secrets.descriptionPlaceholder")}
            value={description()}
            onInput={(e) => setDescription(e.currentTarget.value)}
          />
        </Field>
        <NoticeBox notice={create.notice()} />
        <div class="st-actions">
          <button
            type="submit"
            class="fk-btn st-primary"
            disabled={
              create.busy() || !validName() || (kind() === "database" ? connMissing() : !value())
            }
          >
            {t("settings:secrets.create")}
          </button>
        </div>
      </form>

      <NoticeBox notice={row.notice()} />
      <Show when={list()}>
        {(secrets) => (
          <div class="card">
            <Show
              when={secrets().length > 0}
              fallback={<p class="table__muted">{t("settings:secrets.empty")}</p>}
            >
              <table class="table">
                <thead>
                  <tr>
                    <th>{t("settings:secrets.name")}</th>
                    <th>{t("settings:secrets.updatedAt")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={secrets()}>
                    {(secret) => (
                      <tr>
                        <td>
                          <div class="st-mono">{secret.name}</div>
                          <Show when={secret.kind === "database" && changing() !== secret.id}>
                            <div class="st-conn__summary">
                              {engineById(secret.meta.engine)?.label ?? secret.meta.engine}
                              <Show
                                when={
                                  secret.meta.fields?.host ??
                                  secret.meta.fields?.account ??
                                  secret.meta.fields?.path ??
                                  secret.meta.fields?.url
                                }
                              >
                                {(where) => (
                                  <span class="st-mono">
                                    {" · "}
                                    {where()}
                                    {secret.meta.fields?.database
                                      ? `/${secret.meta.fields.database}`
                                      : ""}
                                  </span>
                                )}
                              </Show>
                            </div>
                          </Show>
                          <Show
                            when={changing() === secret.id}
                            fallback={
                              <Show when={secret.description}>
                                <div class="table__muted">{secret.description}</div>
                              </Show>
                            }
                          >
                            <form
                              class="st-form st-secret-edit"
                              onSubmit={(e) => {
                                e.preventDefault();
                                saveEdit(secret);
                              }}
                            >
                              <Show
                                when={secret.kind === "database"}
                                fallback={
                                  <SecretInput
                                    placeholder={t("settings:secrets.newValue")}
                                    ariaLabel={t("settings:secrets.newValueOf", {
                                      name: secret.name,
                                    })}
                                    value={newValue()}
                                    onInput={setNewValue}
                                    ref={(el) => queueMicrotask(() => el.focus())}
                                  />
                                }
                              >
                                <ConnectionForm
                                  engine={secret.meta.engine ?? ""}
                                  onEngine={() => {}}
                                  lockEngine
                                  fields={editFields()}
                                  onField={(k, v) => setEditFields({ ...editFields(), [k]: v })}
                                  saved={secret.meta.secrets ?? []}
                                />
                              </Show>
                              <input
                                placeholder={t("settings:secrets.description")}
                                aria-label={t("settings:secrets.description")}
                                value={newDescription()}
                                onInput={(e) => setNewDescription(e.currentTarget.value)}
                              />
                              <div class="st-actions">
                                <button
                                  type="button"
                                  class="fk-btn fk-btn--sm"
                                  onClick={() => setChanging(null)}
                                >
                                  {t("settings:secrets.cancel")}
                                </button>
                                <button
                                  type="submit"
                                  class="fk-btn fk-btn--sm st-primary"
                                  disabled={row.busy()}
                                >
                                  {t("settings:secrets.save")}
                                </button>
                              </div>
                            </form>
                          </Show>
                        </td>
                        <td class="table__muted">{formatDateTime(secret.updated_at)}</td>
                        <td class="st-linked__actions">
                          <Show when={changing() !== secret.id}>
                            <button
                              type="button"
                              class="fk-btn fk-btn--sm"
                              disabled={row.busy()}
                              onClick={() => edit(secret)}
                            >
                              {t("settings:secrets.change")}
                            </button>{" "}
                            <button
                              type="button"
                              class="fk-btn fk-btn--sm fk-btn--danger"
                              disabled={row.busy()}
                              onClick={() => setDeleting(secret)}
                            >
                              {t("settings:secrets.delete")}
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
        )}
      </Show>

      <ConfirmDialog
        open={deleting() !== undefined}
        title={t("settings:secrets.confirmTitle")}
        message={t("settings:secrets.confirmBody", { name: deleting()?.name ?? "" })}
        confirmLabel={t("settings:secrets.delete")}
        pending={row.busy()}
        onCancel={() => setDeleting(undefined)}
        onConfirm={remove}
      />
    </section>
  );
};
