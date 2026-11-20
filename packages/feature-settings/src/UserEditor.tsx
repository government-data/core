import { createEffect, createSignal, For, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { iamApi, usersApi, type IamPolicy, type UserAccount } from "./api";
import { Field, NoticeBox, useAction } from "./common";
import { t } from "./i18n";
import { PolicyChecks } from "./iam";

/**
 * Edit an account: its name, its role and the policies attached to it
 * directly (those of its groups are given in Groups). An administrator may
 * do everything; a user, what its policies say (ADR 0016).
 */
export const UserEditor: Component<{
  user?: UserAccount;
  /** The roles an account can have (`admin`, `user`). */
  roles: string[];
  /** Every policy, to attach; without them (no permission), not shown. */
  policies?: IamPolicy[];
  /** The policies attached to the account today. */
  attached: string[];
  /** One's own account: its role does not change. */
  self: boolean;
  roleLabel: (role: string) => string;
  /** Turns the API's errors into the UI's language. */
  said: <T>(call: () => Promise<T>) => Promise<T>;
  onClose: () => void;
  onSaved: (email: string) => void;
}> = (props) => {
  const [name, setName] = createSignal("");
  const [role, setRole] = createSignal("user");
  const [chosen, setChosen] = createSignal<string[]>([]);
  const save = useAction();

  createEffect(() => {
    const user = props.user;
    if (!user) return;
    setName(user.display_name ?? "");
    setRole(user.role);
    setChosen(props.attached);
  });

  const directory = () => props.user?.status === "directory";
  const submit = (e: Event) => {
    e.preventDefault();
    const user = props.user;
    if (!user) return;
    void save.run(async () => {
      await props.said(() =>
        usersApi.update(user.id, { display_name: name().trim() || null, role: role() }),
      );
      if (props.policies) await props.said(() => iamApi.setUserPolicies(user.id, chosen()));
      props.onSaved(user.email);
    });
  };

  return (
    <Modal
      open={props.user !== undefined}
      title={t("settings:users.editTitle", { email: props.user?.email ?? "" })}
      onClose={props.onClose}
      locked={save.busy()}
    >
      <form class="st-form st-user-editor" onSubmit={submit}>
        <Show when={directory()}>
          <p class="st-notice">{t("settings:users.directoryManaged")}</p>
        </Show>
        <div class="st-row">
          <Field label={t("settings:users.name")} error={save.fields().display_name}>
            <input
              value={name()}
              disabled={directory()}
              onInput={(e) => setName(e.currentTarget.value)}
            />
          </Field>
          <Field
            label={t("settings:users.roleLabel")}
            error={save.fields().role}
            hint={
              props.self
                ? t("settings:users.selfRole")
                : t(`settings:users.roleHint.${role() === "admin" ? "admin" : "user"}`)
            }
          >
            <select
              value={role()}
              disabled={props.self || directory()}
              onChange={(e) => setRole(e.currentTarget.value)}
            >
              <For each={props.roles}>{(r) => <option value={r}>{props.roleLabel(r)}</option>}</For>
            </select>
          </Field>
        </div>

        <Show when={props.policies && role() !== "admin"}>
          <fieldset class="st-capabilities">
            <legend>{t("settings:users.policies")}</legend>
            <PolicyChecks policies={props.policies ?? []} chosen={chosen()} onChange={setChosen} />
          </fieldset>
        </Show>
        <NoticeBox notice={save.notice()} />
        <div class="st-actions">
          <button type="button" class="fk-btn" disabled={save.busy()} onClick={props.onClose}>
            {t("settings:users.cancel")}
          </button>
          <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
            {save.busy() ? t("settings:users.saving") : t("settings:users.save")}
          </button>
        </div>
      </form>
    </Modal>
  );
};
