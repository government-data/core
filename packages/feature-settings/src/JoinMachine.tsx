import { createSignal, For, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { formatDateTime } from "@knime-orchestador/i18n";
import { CardIcon } from "./CardIcon";
import { CopyButton, Field, NoticeBox, useAction } from "./common";
import { t } from "./i18n";
import {
  JOIN_ROLES,
  requestJoin,
  takesDrives,
  type JoinOutcome,
  type JoinRole,
} from "./joinRequest";
import { machinesApi, type MachinesApi } from "./machinesApi";

/** The role and the pool of the machine to add, and a lake replica's drives. */
const JoinForm: Component<{
  api: MachinesApi;
  onIssued: (outcome: JoinOutcome) => void;
  onCancel: () => void;
  action: ReturnType<typeof useAction>;
}> = (props) => {
  const [role, setRole] = createSignal<JoinRole>("worker");
  const [pool, setPool] = createSignal("");
  const [drives, setDrives] = createSignal("");

  const submit = (e: Event) => {
    e.preventDefault();
    void props.action.run(async () => {
      props.onIssued(await requestJoin(props.api, role(), pool(), drives()));
    });
  };

  return (
    <form class="st-form" onSubmit={submit}>
      <div class="st-row">
        <Field
          label={t("settings:machines.join.role")}
          error={props.action.fields().role}
          hint={t("settings:machines.join.roleHint")}
        >
          <select value={role()} onChange={(e) => setRole(e.currentTarget.value as JoinRole)}>
            <For each={JOIN_ROLES}>{(r) => <option value={r}>{r}</option>}</For>
          </select>
        </Field>
        <Field
          label={t("settings:machines.join.pool")}
          error={props.action.fields().pool}
          hint={t("settings:machines.join.poolHint")}
        >
          <input
            value={pool()}
            spellcheck={false}
            autocomplete="off"
            onInput={(e) => setPool(e.currentTarget.value)}
          />
        </Field>
      </div>
      <Show when={takesDrives(role())}>
        <Field
          label={t("settings:machines.join.drives")}
          error={props.action.fields().drives}
          hint={t("settings:machines.join.drivesHint")}
        >
          <textarea
            rows="3"
            value={drives()}
            spellcheck={false}
            autocomplete="off"
            placeholder={"/srv/disk1\n/srv/disk2"}
            onInput={(e) => setDrives(e.currentTarget.value)}
          />
        </Field>
      </Show>
      <NoticeBox notice={props.action.notice()} />
      <div class="st-actions">
        <button
          type="button"
          class="fk-btn"
          disabled={props.action.busy()}
          onClick={() => props.onCancel()}
        >
          {t("settings:machines.join.cancel")}
        </button>
        <button type="submit" class="fk-btn st-primary" disabled={props.action.busy()}>
          {props.action.busy()
            ? t("settings:machines.join.issuing")
            : t("settings:machines.join.submit")}
        </button>
      </div>
    </form>
  );
};

/** The secret, once: the command to run, or the bare token and why there is no command (also a lake pool node's, in Settings → Lake). */
export const JoinResult: Component<{ outcome: JoinOutcome; onDone: () => void }> = (props) => {
  const secret = () =>
    props.outcome.kind === "command" ? props.outcome.command : props.outcome.token;
  return (
    <div class="st-form">
      <p class="st-notice st-notice--warn">{t("settings:machines.join.once")}</p>
      <p>
        {props.outcome.pool
          ? t("settings:machines.join.for", { role: props.outcome.role, pool: props.outcome.pool })
          : t("settings:machines.join.forNoPool", { role: props.outcome.role })}{" "}
        {t("settings:machines.join.expires", {
          at: formatDateTime(props.outcome.expiresAt * 1000),
        })}
      </p>
      <Show
        when={props.outcome.kind === "command"}
        fallback={<p>{t("settings:machines.join.noCommand", { role: props.outcome.role })}</p>}
      >
        <p>{t("settings:machines.join.runAsRoot")}</p>
        <Show when={props.outcome.kind === "command" && props.outcome.addDrives}>
          <p>{t("settings:machines.join.addDrives")}</p>
        </Show>
      </Show>
      <div class="st-command">
        <span class="st-field__label">
          {props.outcome.kind === "command"
            ? t("settings:machines.join.command")
            : t("settings:machines.join.token")}
        </span>
        <pre class="st-mono st-command__text">{secret()}</pre>
        <CopyButton text={secret()} />
      </div>
      <div class="st-actions">
        <button type="button" class="fk-btn st-primary" onClick={() => props.onDone()}>
          {t("settings:machines.join.done")}
        </button>
      </div>
    </div>
  );
};

/**
 * "Add a machine" (decisions 0029 and 0066 step 11): the role and the pool
 * (and a lake replica's drives, or none to add by hand), then the command the
 * API built, shown once. The answer, which holds the
 * token, lives only in this component's state: never logged, stored or put
 * in a URL, and dropped when the panel closes; `onClosed` then reads the
 * machines again. While it is shown the panel closes only with its button,
 * so a stray click or Escape does not lose it.
 */
export const JoinMachine: Component<{ api?: MachinesApi; onClosed: () => void }> = (props) => {
  const api = () => props.api ?? machinesApi;
  const [open, setOpen] = createSignal(false);
  const [outcome, setOutcome] = createSignal<JoinOutcome>();
  const action = useAction();

  const close = () => {
    const issued = outcome() !== undefined;
    setOutcome(undefined);
    setOpen(false);
    if (issued) props.onClosed();
  };

  return (
    <>
      <div class="st-toolbar">
        <button
          type="button"
          class="fk-btn st-primary st-toolbar__button"
          onClick={() => setOpen(true)}
        >
          <CardIcon name="server" size={18} />
          {t("settings:machines.join.add")}
        </button>
      </div>
      <Modal
        open={open()}
        title={t("settings:machines.join.add")}
        onClose={close}
        locked={action.busy() || outcome() !== undefined}
      >
        <Show
          when={outcome()}
          fallback={<JoinForm api={api()} action={action} onIssued={setOutcome} onCancel={close} />}
        >
          {(issued) => <JoinResult outcome={issued()} onDone={close} />}
        </Show>
      </Modal>
    </>
  );
};
