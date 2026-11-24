/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Index, Show, type Component } from "solid-js";
import { ConfirmDialog, Modal } from "@knime-orchestador/feature-kit/ui";
import { iamApi, SettingsApiError, type IamPolicy, type IamStatement } from "./api";
import { Field, lines, NoticeBox, Panel, useAction } from "./common";
import { t } from "./i18n";
import { CardIcon } from "./CardIcon";
import { actionLabel } from "./iam";
import { PoliciesTable } from "./PoliciesTable";

const newStatement = (): IamStatement => ({ effect: "Allow", actions: [], resources: ["*"] });

/**
 * Settings → Policies (ADR 0016): the managed ones, which come with the
 * application and cannot be changed, and the custom ones: statements that
 * allow or deny actions on resources (`*`, `secret/BD_*`…). A deny always
 * wins.
 */
export const PoliciesPage: Component<{ embedded?: boolean }> = (props) => {
  const [policies, { refetch }] = createResource(iamApi.policies);
  const [catalog] = createResource(iamApi.catalog);
  const [editing, setEditing] = createSignal<IamPolicy | "new">();
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [statements, setStatements] = createSignal<IamStatement[]>([]);
  const [deleting, setDeleting] = createSignal<IamPolicy>();
  const save = useAction();
  const row = useAction();

  const open = (p?: IamPolicy) => {
    setName(p?.name ?? "");
    setDescription(p?.description ?? "");
    setStatements(p ? p.statements.map((s) => ({ ...s })) : [newStatement()]);
    setEditing(p ?? "new");
  };
  const change = (i: number, patch: Partial<IamStatement>) =>
    setStatements((all) => all.map((s, j) => (j === i ? { ...s, ...patch } : s)));

  const submit = (e: Event) => {
    e.preventDefault();
    void save.run(async () => {
      await iamApi.putPolicy(name().trim(), {
        description: description(),
        statements: statements(),
      });
      setEditing(undefined);
      await refetch();
      return t("settings:policies.saved", { name: name().trim() });
    });
  };

  const remove = () => {
    const p = deleting();
    if (!p) return;
    void row
      .run(async () => {
        await iamApi.deletePolicy(p.name);
        await refetch();
        return t("settings:policies.deleted", { name: p.name });
      })
      .finally(() => setDeleting(undefined));
  };

  /** An action's name in the UI's language: a module's says what it does. */
  const label = (action: string) =>
    catalog()?.actions.find((a) => a.name === action)?.description ?? actionLabel(action);
  /** A service's name (`quality`, `admin`); a module's, as it is named. */
  const serviceLabel = (service: string) => {
    const key = `settings:policies.service.${service}`;
    const text = t(key);
    return text === key || text === key.slice("settings:".length) ? service : text;
  };

  /** The actions of a service (`quality`, `admin`, a module), for the checkboxes. */
  const services = () => {
    const by = new Map<string, string[]>();
    for (const a of catalog()?.actions ?? []) {
      const service = a.name.split(":")[0];
      by.set(service, [...(by.get(service) ?? []), a.name]);
    }
    return [...by.entries()];
  };

  return (
    <Panel
      embedded={props.embedded}
      title={t("settings:policies.title")}
      subtitle={t("settings:policies.subtitle")}
    >
      <Show when={policies.error}>
        <p class="st-notice st-notice--error">
          {policies.error instanceof SettingsApiError
            ? policies.error.message
            : t("settings:policies.loadError")}
        </p>
      </Show>
      <div class="st-toolbar">
        <button
          type="button"
          class="fk-btn st-primary st-toolbar__button"
          disabled={!catalog()}
          onClick={() => open()}
        >
          <CardIcon name="policy" size={18} />
          {t("settings:policies.new")}
        </button>
      </div>
      <NoticeBox notice={row.notice()} />
      <Show when={policies()}>
        {(list) => (
          <PoliciesTable
            policies={list()}
            busy={row.busy()}
            editable={!!catalog()}
            onEdit={open}
            onDelete={setDeleting}
          />
        )}
      </Show>

      <Modal
        open={editing() !== undefined}
        title={editing() === "new" ? t("settings:policies.new") : t("settings:policies.edit")}
        onClose={() => setEditing(undefined)}
        locked={save.busy()}
      >
        <form class="st-form" onSubmit={submit}>
          <div class="st-row">
            <Field
              label={t("settings:policies.name")}
              error={save.fields().name}
              hint={t("settings:iam.nameHint")}
            >
              <input
                required
                disabled={editing() !== "new"}
                value={name()}
                onInput={(e) => setName(e.currentTarget.value)}
              />
            </Field>
            <Field label={t("settings:policies.description")} error={save.fields().description}>
              <input value={description()} onInput={(e) => setDescription(e.currentTarget.value)} />
            </Field>
          </div>
          <Index each={statements()}>
            {(s, i) => (
              <fieldset class="st-capabilities st-statement">
                <legend>{t("settings:policies.statement", { n: i + 1 })}</legend>
                <div class="st-row">
                  <Field label={t("settings:policies.effect")}>
                    <select
                      value={s().effect}
                      onChange={(e) =>
                        change(i, { effect: e.currentTarget.value as IamStatement["effect"] })
                      }
                    >
                      <option value="Allow">{t("settings:policies.allow")}</option>
                      <option value="Deny">{t("settings:policies.deny")}</option>
                    </select>
                  </Field>
                  <Field
                    label={t("settings:policies.resources")}
                    hint={t("settings:policies.resourcesHint")}
                  >
                    <textarea
                      rows={2}
                      class="st-mono"
                      value={s().resources.join("\n")}
                      onInput={(e) => change(i, { resources: lines(e.currentTarget.value) })}
                    />
                  </Field>
                </div>
                <label class="st-check">
                  <input
                    type="checkbox"
                    checked={s().actions.includes("*")}
                    onChange={(e) => change(i, { actions: e.currentTarget.checked ? ["*"] : [] })}
                  />
                  <span>{t("settings:policies.everything")}</span>
                </label>
                <Show when={!s().actions.includes("*")}>
                  <For each={services()}>
                    {([service, actions]) => (
                      <div class="st-action-group">
                        <span class="st-field__label">{serviceLabel(service)}</span>
                        <div class="st-policy-checks st-policy-checks--grid">
                          <For each={actions}>
                            {(a) => (
                              <label class="st-check">
                                <input
                                  type="checkbox"
                                  checked={s().actions.includes(a)}
                                  onChange={(e) =>
                                    change(i, {
                                      actions: e.currentTarget.checked
                                        ? [...s().actions, a]
                                        : s().actions.filter((x) => x !== a),
                                    })
                                  }
                                />
                                <span>{label(a)}</span>
                              </label>
                            )}
                          </For>
                        </div>
                      </div>
                    )}
                  </For>
                </Show>
                <Show when={statements().length > 1}>
                  <button
                    type="button"
                    class="fk-btn fk-btn--sm fk-btn--danger"
                    onClick={() => setStatements((all) => all.filter((_, j) => j !== i))}
                  >
                    {t("settings:policies.removeStatement")}
                  </button>
                </Show>
              </fieldset>
            )}
          </Index>
          <button
            type="button"
            class="fk-btn fk-btn--sm"
            onClick={() => setStatements((all) => [...all, newStatement()])}
          >
            {t("settings:policies.addStatement")}
          </button>
          <Show when={save.fields().statements || save.fields().actions || save.fields().resources}>
            {(e) => <p class="st-notice st-notice--error">{e()}</p>}
          </Show>
          <NoticeBox notice={save.notice()} />
          <div class="st-actions">
            <button
              type="button"
              class="fk-btn"
              disabled={save.busy()}
              onClick={() => setEditing(undefined)}
            >
              {t("settings:users.cancel")}
            </button>
            <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
              {save.busy() ? t("settings:users.saving") : t("settings:users.save")}
            </button>
          </div>
        </form>
      </Modal>

      <ConfirmDialog
        open={deleting() !== undefined}
        title={t("settings:policies.confirmTitle")}
        message={t("settings:policies.confirmBody", { name: deleting()?.name ?? "" })}
        confirmLabel={t("settings:users.delete")}
        pending={row.busy()}
        onCancel={() => setDeleting(undefined)}
        onConfirm={remove}
      />
    </Panel>
  );
};
