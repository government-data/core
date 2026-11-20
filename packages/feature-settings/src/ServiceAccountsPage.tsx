/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { ConfirmDialog, Modal } from "@knime-orchestador/feature-kit/ui";
import { iamApi, SettingsApiError, type ServiceAccount } from "./api";
import { CopyButton, Field, NoticeBox, Panel, useAction } from "./common";
import { t } from "./i18n";
import { CardIcon } from "./CardIcon";
import { PolicyChecks, PolicyChips } from "./iam";

const dateTime = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "—");

/**
 * Settings → Service accounts (ADR 0016): tokens for integrations and
 * scripts, which call the API with `Authorization: Bearer <token>`. Each
 * belongs to whoever creates it and can do at most what they can; with
 * policies of its own, only what both allow. The token is shown once.
 */
export const ServiceAccountsPage: Component<{ embedded?: boolean }> = (props) => {
  const [list, { refetch }] = createResource(iamApi.serviceAccounts);
  // Whoever cannot list the policies creates them without (their own rights).
  const [policies] = createResource(() => iamApi.policies().catch(() => []));
  const [creating, setCreating] = createSignal(false);
  const [name, setName] = createSignal("");
  const [expires, setExpires] = createSignal("");
  const [chosen, setChosen] = createSignal<string[]>([]);
  const [token, setToken] = createSignal<string>();
  const [deleting, setDeleting] = createSignal<ServiceAccount>();
  const save = useAction();
  const row = useAction();

  const open = () => {
    setName("");
    setExpires("");
    setChosen([]);
    setCreating(true);
  };

  const submit = (e: Event) => {
    e.preventDefault();
    void save.run(async () => {
      const created = await iamApi.createServiceAccount({
        name: name().trim(),
        policies: chosen(),
        expires_at: expires() ? new Date(`${expires()}T23:59:59`).toISOString() : null,
      });
      setCreating(false);
      setToken(created.token);
      await refetch();
    });
  };

  const remove = () => {
    const sa = deleting();
    if (!sa) return;
    void row
      .run(async () => {
        await iamApi.deleteServiceAccount(sa.id);
        await refetch();
        return t("settings:serviceAccounts.deleted", { name: sa.name });
      })
      .finally(() => setDeleting(undefined));
  };

  return (
    <Panel
      embedded={props.embedded}
      title={t("settings:serviceAccounts.title")}
      subtitle={t("settings:serviceAccounts.subtitle")}
    >
      <Show when={list.error}>
        <p class="st-notice st-notice--error">
          {list.error instanceof SettingsApiError
            ? list.error.message
            : t("settings:serviceAccounts.loadError")}
        </p>
      </Show>
      <div class="st-toolbar">
        <button type="button" class="fk-btn st-primary st-toolbar__button" onClick={open}>
          <CardIcon name="key" size={18} />
          {t("settings:serviceAccounts.new")}
        </button>
      </div>
      <Show when={token()}>
        {(tk) => (
          <div class="st-notice st-notice--warn st-token">
            <p>{t("settings:serviceAccounts.tokenOnce")}</p>
            <code class="st-mono">{tk()}</code> <CopyButton text={tk()} />{" "}
            <button type="button" class="fk-btn fk-btn--sm" onClick={() => setToken(undefined)}>
              {t("settings:serviceAccounts.done")}
            </button>
          </div>
        )}
      </Show>
      <NoticeBox notice={row.notice()} />
      <Show when={list()}>
        {(items) => (
          <div class="card">
            <Show
              when={items().length > 0}
              fallback={<p class="table__muted">{t("settings:serviceAccounts.empty")}</p>}
            >
              <table class="table">
                <thead>
                  <tr>
                    <th>{t("settings:serviceAccounts.name")}</th>
                    <th>{t("settings:serviceAccounts.owner")}</th>
                    <th>{t("settings:serviceAccounts.policies")}</th>
                    <th>{t("settings:serviceAccounts.lastUsed")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={items()}>
                    {(sa) => (
                      <tr>
                        <td>
                          <div>{sa.name}</div>
                          <div class="table__muted st-mono">{sa.access_key}</div>
                          <Show when={sa.expires_at}>
                            <div class="table__muted">
                              {t("settings:serviceAccounts.expires", {
                                at: dateTime(sa.expires_at),
                              })}
                            </div>
                          </Show>
                        </td>
                        <td>{sa.owner_email}</td>
                        <td>
                          <PolicyChips
                            names={sa.policies}
                            empty={t("settings:serviceAccounts.ownerPolicies")}
                          />
                        </td>
                        <td class="table__muted">{dateTime(sa.last_used_at)}</td>
                        <td class="st-linked__actions">
                          <button
                            type="button"
                            class="fk-btn fk-btn--sm fk-btn--danger"
                            disabled={row.busy()}
                            onClick={() => setDeleting(sa)}
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
        open={creating()}
        title={t("settings:serviceAccounts.new")}
        onClose={() => setCreating(false)}
        locked={save.busy()}
      >
        <form class="st-form" onSubmit={submit}>
          <div class="st-row">
            <Field label={t("settings:serviceAccounts.name")} error={save.fields().name}>
              <input required value={name()} onInput={(e) => setName(e.currentTarget.value)} />
            </Field>
            <Field
              label={t("settings:serviceAccounts.expiresAt")}
              error={save.fields().expires_at}
              hint={t("settings:serviceAccounts.expiresHint")}
            >
              <input
                type="date"
                value={expires()}
                onInput={(e) => setExpires(e.currentTarget.value)}
              />
            </Field>
          </div>
          <Show when={(policies() ?? []).length > 0}>
            <fieldset class="st-capabilities">
              <legend>{t("settings:serviceAccounts.policies")}</legend>
              <p class="table__muted">{t("settings:serviceAccounts.policiesHint")}</p>
              <PolicyChecks policies={policies() ?? []} chosen={chosen()} onChange={setChosen} />
            </fieldset>
          </Show>
          <NoticeBox notice={save.notice()} />
          <div class="st-actions">
            <button
              type="button"
              class="fk-btn"
              disabled={save.busy()}
              onClick={() => setCreating(false)}
            >
              {t("settings:users.cancel")}
            </button>
            <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
              {save.busy() ? t("settings:users.saving") : t("settings:serviceAccounts.create")}
            </button>
          </div>
        </form>
      </Modal>

      <ConfirmDialog
        open={deleting() !== undefined}
        title={t("settings:serviceAccounts.confirmTitle")}
        message={t("settings:serviceAccounts.confirmBody", { name: deleting()?.name ?? "" })}
        confirmLabel={t("settings:users.delete")}
        pending={row.busy()}
        onCancel={() => setDeleting(undefined)}
        onConfirm={remove}
      />
    </Panel>
  );
};
