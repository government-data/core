/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { ConfirmDialog, Modal } from "@knime-orchestador/feature-kit/ui";
import { iamApi, SettingsApiError, usersApi, type IamGroup, type IamGroupChange } from "./api";
import { Field, lines, NoticeBox, Panel, useAction } from "./common";
import { t } from "./i18n";
import { CardIcon } from "./CardIcon";
import { PolicyChecks, PolicyChips } from "./iam";

const EMPTY: IamGroupChange = {
  name: "",
  description: "",
  directory_groups: [],
  members: [],
  policies: [],
};

/**
 * Settings → Groups (ADR 0016): its people get its policies. People are
 * added by hand or come from directory groups (LDAP / AD) at every sign-in.
 */
export const GroupsPage: Component<{ embedded?: boolean }> = (props) => {
  const [groups, { refetch }] = createResource(iamApi.groups);
  const [policies] = createResource(iamApi.policies);
  const [users] = createResource(() => usersApi.list().then((l) => l.users));
  const [editing, setEditing] = createSignal<IamGroup | "new">();
  const [form, setForm] = createSignal<IamGroupChange>(EMPTY);
  const [deleting, setDeleting] = createSignal<IamGroup>();
  const save = useAction();
  const row = useAction();

  const open = (g?: IamGroup) => {
    setForm(
      g
        ? {
            name: g.name,
            description: g.description,
            directory_groups: g.directory_groups,
            members: g.members.filter((m) => m.source === "manual").map((m) => m.admin_id),
            policies: g.policies,
          }
        : EMPTY,
    );
    setEditing(g ?? "new");
  };
  const set = (patch: Partial<IamGroupChange>) => setForm((f) => ({ ...f, ...patch }));

  const submit = (e: Event) => {
    e.preventDefault();
    const target = editing();
    void save.run(async () => {
      if (target === "new") await iamApi.createGroup(form());
      else if (target) await iamApi.updateGroup(target.id, form());
      setEditing(undefined);
      await refetch();
      return t("settings:groups.saved", { name: form().name });
    });
  };

  const remove = () => {
    const g = deleting();
    if (!g) return;
    void row
      .run(async () => {
        await iamApi.deleteGroup(g.id);
        await refetch();
        return t("settings:groups.deleted", { name: g.name });
      })
      .finally(() => setDeleting(undefined));
  };

  return (
    <Panel
      embedded={props.embedded}
      title={t("settings:groups.title")}
      subtitle={t("settings:groups.subtitle")}
    >
      <Show when={groups.error}>
        <p class="st-notice st-notice--error">
          {groups.error instanceof SettingsApiError
            ? groups.error.message
            : t("settings:groups.loadError")}
        </p>
      </Show>
      <div class="st-toolbar">
        <button
          type="button"
          class="fk-btn st-primary st-toolbar__button"
          disabled={!groups() || !policies()}
          onClick={() => open()}
        >
          <CardIcon name="group" size={18} />
          {t("settings:groups.new")}
        </button>
      </div>
      <NoticeBox notice={row.notice()} />
      <Show when={groups()}>
        {(list) => (
          <div class="card">
            <Show
              when={list().length > 0}
              fallback={<p class="table__muted">{t("settings:groups.empty")}</p>}
            >
              <table class="table">
                <thead>
                  <tr>
                    <th>{t("settings:groups.name")}</th>
                    <th>{t("settings:groups.members")}</th>
                    <th>{t("settings:groups.policies")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={list()}>
                    {(g) => (
                      <tr>
                        <td>
                          <div>{g.name}</div>
                          <Show when={g.description}>
                            <div class="table__muted">{g.description}</div>
                          </Show>
                          <For each={g.directory_groups}>
                            {(dn) => <div class="table__muted st-mono">{dn}</div>}
                          </For>
                        </td>
                        <td>
                          <PolicyChips
                            names={g.members.map(
                              (m) =>
                                (m.display_name || m.email) +
                                (m.source === "directory" ? " · LDAP" : ""),
                            )}
                            empty={t("settings:groups.nobody")}
                          />
                        </td>
                        <td>
                          <PolicyChips names={g.policies} />
                        </td>
                        <td class="st-linked__actions">
                          <button
                            type="button"
                            class="fk-btn fk-btn--sm"
                            disabled={row.busy() || !policies()}
                            onClick={() => open(g)}
                          >
                            {t("settings:users.edit")}
                          </button>{" "}
                          <button
                            type="button"
                            class="fk-btn fk-btn--sm fk-btn--danger"
                            disabled={row.busy()}
                            onClick={() => setDeleting(g)}
                          >
                            {t("settings:users.delete")}
                          </button>
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

      <Modal
        open={editing() !== undefined}
        title={editing() === "new" ? t("settings:groups.new") : t("settings:groups.edit")}
        onClose={() => setEditing(undefined)}
        locked={save.busy()}
      >
        <form class="st-form" onSubmit={submit}>
          <div class="st-row">
            <Field
              label={t("settings:groups.name")}
              error={save.fields().name}
              hint={t("settings:iam.nameHint")}
            >
              <input
                required
                value={form().name}
                onInput={(e) => set({ name: e.currentTarget.value })}
              />
            </Field>
            <Field label={t("settings:groups.description")} error={save.fields().description}>
              <input
                value={form().description}
                onInput={(e) => set({ description: e.currentTarget.value })}
              />
            </Field>
          </div>
          <Field
            label={t("settings:groups.directoryGroups")}
            error={save.fields().directory_groups}
            hint={t("settings:groups.directoryHint")}
          >
            <textarea
              rows={3}
              class="st-mono"
              value={form().directory_groups.join("\n")}
              onInput={(e) => set({ directory_groups: lines(e.currentTarget.value) })}
            />
          </Field>
          <fieldset class="st-capabilities">
            <legend>{t("settings:groups.manualMembers")}</legend>
            <div class="st-policy-checks">
              <For each={users() ?? []}>
                {(u) => (
                  <label class="st-check">
                    <input
                      type="checkbox"
                      checked={form().members.includes(u.id)}
                      onChange={(e) =>
                        set({
                          members: e.currentTarget.checked
                            ? [...form().members, u.id]
                            : form().members.filter((m) => m !== u.id),
                        })
                      }
                    />
                    <span>{u.display_name || u.email}</span>
                  </label>
                )}
              </For>
            </div>
          </fieldset>
          <fieldset class="st-capabilities">
            <legend>{t("settings:groups.policies")}</legend>
            <PolicyChecks
              policies={policies() ?? []}
              chosen={form().policies}
              onChange={(policies) => set({ policies })}
            />
          </fieldset>
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
        title={t("settings:groups.confirmTitle")}
        message={t("settings:groups.confirmBody", { name: deleting()?.name ?? "" })}
        confirmLabel={t("settings:users.delete")}
        pending={row.busy()}
        onCancel={() => setDeleting(undefined)}
        onConfirm={remove}
      />
    </Panel>
  );
};
