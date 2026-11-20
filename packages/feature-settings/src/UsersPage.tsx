/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { A } from "@solidjs/router";
import { useSession } from "@knime-orchestador/feature-kit";
import { ConfirmDialog, Modal } from "@knime-orchestador/feature-kit/ui";
import { iamApi, SettingsApiError, usersApi, type UserAccount } from "./api";
import { Field, NoticeBox, Panel, useAction } from "./common";
import { t } from "./i18n";
import { CardIcon } from "./CardIcon";
import { PolicyChips } from "./iam";
import { UserEditor } from "./UserEditor";

/** «Administrador», «Usuario»… (or the role as is if it is not one of ours). */
const roleLabel = (role: string) =>
  role === "admin" || role === "user" ? t(`settings:users.role.${role}`) : role;

/** The API's errors, in the UI's language. */
const USER_ERRORS = [
  "email_taken",
  "already_active",
  "self_delete",
  "last_admin",
  "mail_not_configured",
  "mail_failed",
  "self_role",
  "directory_managed",
  "admin_only",
];

async function said<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call();
  } catch (e) {
    if (e instanceof SettingsApiError) {
      const fields = Object.fromEntries(
        Object.keys(e.fields).map((f) => [f, t(`settings:users.invalid.${f}`)]),
      );
      const message = USER_ERRORS.includes(e.code)
        ? t(`settings:users.errors.${e.code}`)
        : e.message;
      throw new SettingsApiError(e.status, message, fields, e.code);
    }
    throw e;
  }
}

/**
 * Settings → Users: the company's accounts, creating a new one and what each
 * may do: `admin` everything; anyone else, what its policies and its groups'
 * give (ADR 0016). Whoever is created here gets an email with a link to
 * choose their password; until then the account is pending and cannot be
 * signed in to.
 */
export const UsersPage: Component<{ embedded?: boolean }> = (props) => {
  const session = useSession();
  const [list, { refetch }] = createResource(usersApi.list);
  const [editing, setEditing] = createSignal<UserAccount>();
  const [email, setEmail] = createSignal("");
  const [name, setName] = createSignal("");
  const [role, setRole] = createSignal("user");
  const [deleting, setDeleting] = createSignal<UserAccount>();
  const create = useAction();
  const row = useAction();
  const [creatingUser, setCreatingUser] = createSignal(false);
  // What each account has, and the policies to give (whoever cannot see
  // them, does not get the column).
  const [access, { refetch: refetchAccess }] = createResource(() =>
    iamApi.users().catch(() => undefined),
  );
  const [policies] = createResource(() => iamApi.policies().catch(() => undefined));
  const accessOf = (id: string) => access()?.find((a) => a.admin_id === id);
  /** What a role is for, for the hint under the role select. */
  const roleHint = (r: string) => t(`settings:users.roleHint.${r === "admin" ? "admin" : "user"}`);

  const submit = (e: Event) => {
    e.preventDefault();
    void create.run(async () => {
      const to = email().trim();
      const out = await said(() =>
        usersApi.create({ email: to, display_name: name().trim() || null, role: role() }),
      );
      setEmail("");
      setName("");
      setCreatingUser(false);
      await refetch();
      return out.activation_sent
        ? t("settings:users.created", { email: to })
        : t("settings:users.createdNoMail", { email: to });
    });
  };

  const resend = (user: UserAccount) =>
    void row.run(async () => {
      await said(() => usersApi.resend(user.id));
      return t("settings:users.resent", { email: user.email });
    });

  const remove = () => {
    const user = deleting();
    if (!user) return;
    void row
      .run(async () => {
        await said(() => usersApi.remove(user.id));
        await refetch();
        return t("settings:users.deleted", { email: user.email });
      })
      .finally(() => setDeleting(undefined));
  };

  const saved = (email: string) => {
    setEditing(undefined);
    void refetch();
    void refetchAccess();
    void row.run(async () => t("settings:users.updated", { email }));
  };

  return (
    <Panel
      embedded={props.embedded}
      title={t("settings:users.title")}
      subtitle={t("settings:users.subtitle")}
    >
      <Show when={list.error}>
        <p class="st-notice st-notice--error">
          {list.error instanceof SettingsApiError
            ? list.error.message
            : t("settings:users.loadError")}
        </p>
      </Show>

      <div class="st-toolbar">
        <button
          type="button"
          class="fk-btn st-primary st-toolbar__button"
          disabled={!list()}
          onClick={() => setCreatingUser(true)}
        >
          <CardIcon name="userPlus" size={18} />
          {t("settings:users.register")}
        </button>
      </div>

      <Show when={list()}>
        {(data) => (
          <>
            <Modal
              open={creatingUser()}
              title={t("settings:users.new")}
              onClose={() => setCreatingUser(false)}
              locked={create.busy()}
            >
              <form class="st-form" onSubmit={submit}>
                <Show when={!data().mail_configured}>
                  <p class="st-notice st-notice--warn">
                    {t("settings:users.needsMail")}{" "}
                    <A href="/settings/mail">{t("settings:users.configureMail")}</A>
                  </p>
                </Show>
                <div class="st-row">
                  <Field label={t("settings:users.email")} error={create.fields().email}>
                    <input
                      type="email"
                      required
                      autocomplete="off"
                      value={email()}
                      onInput={(e) => setEmail(e.currentTarget.value)}
                    />
                  </Field>
                  <Field label={t("settings:users.name")} error={create.fields().display_name}>
                    <input value={name()} onInput={(e) => setName(e.currentTarget.value)} />
                  </Field>
                </div>
                <Field
                  label={t("settings:users.roleLabel")}
                  error={create.fields().role}
                  hint={roleHint(role())}
                >
                  <select value={role()} onChange={(e) => setRole(e.currentTarget.value)}>
                    <For each={data().roles}>
                      {(r) => <option value={r}>{roleLabel(r)}</option>}
                    </For>
                  </select>
                </Field>
                <NoticeBox notice={create.notice()} />
                <div class="st-actions">
                  <button
                    type="button"
                    class="fk-btn"
                    disabled={create.busy()}
                    onClick={() => setCreatingUser(false)}
                  >
                    {t("settings:users.cancel")}
                  </button>
                  <button
                    type="submit"
                    class="fk-btn st-primary"
                    disabled={create.busy() || !data().mail_configured}
                  >
                    {create.busy() ? t("settings:users.creating") : t("settings:users.create")}
                  </button>
                </div>
              </form>
            </Modal>

            <NoticeBox notice={row.notice()} />
            <div class="card">
              <table class="table">
                <thead>
                  <tr>
                    <th>{t("settings:users.person")}</th>
                    <th>{t("settings:users.roleLabel")}</th>
                    <Show when={access()}>
                      <th>{t("settings:users.access")}</th>
                    </Show>
                    <th>{t("settings:users.status")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={data().users}>
                    {(user) => (
                      <tr>
                        <td>
                          <div>{user.display_name || user.email}</div>
                          <Show when={user.display_name}>
                            <div class="table__muted">{user.email}</div>
                          </Show>
                        </td>
                        <td>{roleLabel(user.role)}</td>
                        <Show when={access()}>
                          <td>
                            <Show
                              when={user.role !== "admin"}
                              fallback={
                                <span class="table__muted">{t("settings:users.everything")}</span>
                              }
                            >
                              <PolicyChips
                                names={[
                                  ...(accessOf(user.id)?.policies ?? []),
                                  ...(accessOf(user.id)?.groups ?? []).map(
                                    (g) => `${t("settings:users.groupPrefix")} ${g}`,
                                  ),
                                ]}
                                empty={t("settings:users.nothing")}
                              />
                            </Show>
                          </td>
                        </Show>
                        <td>
                          <span
                            class="badge"
                            classList={{ "badge--muted": user.status !== "active" }}
                          >
                            {t(`settings:users.statusLabel.${user.status}`)}
                          </span>
                        </td>
                        <td class="st-linked__actions">
                          <Show when={user.status === "pending"}>
                            <button
                              type="button"
                              class="fk-btn fk-btn--sm"
                              disabled={row.busy() || !data().mail_configured}
                              onClick={() => resend(user)}
                            >
                              {t("settings:users.resend")}
                            </button>{" "}
                          </Show>
                          <button
                            type="button"
                            class="fk-btn fk-btn--sm"
                            disabled={row.busy()}
                            onClick={() => setEditing(user)}
                          >
                            {t("settings:users.edit")}
                          </button>{" "}
                          <Show when={user.id !== session.admin_id}>
                            <button
                              type="button"
                              class="fk-btn fk-btn--sm fk-btn--danger"
                              disabled={row.busy()}
                              onClick={() => setDeleting(user)}
                            >
                              {t("settings:users.delete")}
                            </button>
                          </Show>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </>
        )}
      </Show>

      <UserEditor
        user={editing()}
        roles={list()?.roles ?? []}
        policies={policies()}
        attached={accessOf(editing()?.id ?? "")?.policies ?? []}
        self={editing()?.id === session.admin_id}
        roleLabel={roleLabel}
        said={said}
        onClose={() => setEditing(undefined)}
        onSaved={saved}
      />

      <ConfirmDialog
        open={deleting() !== undefined}
        title={t("settings:users.confirmTitle")}
        message={t("settings:users.confirmBody", { email: deleting()?.email ?? "" })}
        confirmLabel={t("settings:users.delete")}
        pending={row.busy()}
        onCancel={() => setDeleting(undefined)}
        onConfirm={remove}
      />
    </Panel>
  );
};
