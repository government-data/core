import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { SettingsApiError } from "./api";
import { Field, NoticeBox, useAction } from "./common";
import { t } from "./i18n";
import { JoinResult } from "./JoinMachine";
import type { JoinOutcome } from "./joinRequest";
import {
  defineMainPool,
  definable,
  growsOnlyByCopy,
  loadMainPool,
  nodeState,
  parseNodes,
  requestNodeJoin,
} from "./lakePools";
import type { LakePool, MachinesApi } from "./machinesApi";

/** The pool's layout, typed: its nodes in order and the drives of each. */
const PoolForm: Component<{
  api: MachinesApi;
  pool?: LakePool;
  onDefined: (pool: LakePool) => void;
}> = (props) => {
  const [nodes, setNodes] = createSignal(props.pool?.nodes.map((n) => n.name).join("\n") ?? "");
  const [drives, setDrives] = createSignal(String(props.pool?.drives ?? 1));
  const action = useAction();
  const submit = (e: Event) => {
    e.preventDefault();
    void action.run(async () => {
      props.onDefined(await defineMainPool(props.api, nodes(), Number(drives())));
      return t("settings:lake.pool.defined");
    });
  };
  return (
    <form class="st-form" onSubmit={submit}>
      <Field
        label={t("settings:lake.pool.nodes")}
        error={action.fields().nodes ?? action.fields().name}
        hint={t("settings:lake.pool.nodesHint")}
      >
        <textarea
          rows="4"
          value={nodes()}
          spellcheck={false}
          autocomplete="off"
          placeholder={"lake-1\nlake-2\nlake-3\nlake-4"}
          onInput={(e) => setNodes(e.currentTarget.value)}
        />
      </Field>
      <Field
        label={t("settings:lake.pool.drives")}
        error={action.fields().drives}
        hint={t("settings:lake.pool.drivesHint")}
      >
        <input
          type="number"
          min="1"
          max="16"
          value={drives()}
          onInput={(e) => setDrives(e.currentTarget.value)}
        />
      </Field>
      <Show when={growsOnlyByCopy(parseNodes(nodes()).length, Number(drives()))}>
        <p class="st-notice st-notice--warn">{t("settings:lake.pool.oneDrive")}</p>
      </Show>
      <NoticeBox notice={action.notice()} />
      <div class="st-actions">
        <button type="submit" class="fk-btn st-primary" disabled={action.busy()}>
          {action.busy() ? t("settings:lake.pool.saving") : t("settings:lake.pool.save")}
        </button>
      </div>
    </form>
  );
};

/**
 * A node's command, asked with its drives or none, then shown once. The
 * answer holds the token: it lives in this component's state alone and is
 * dropped when the panel closes, which only its button does once shown.
 */
const NodeCommand: Component<{ api: MachinesApi; node?: string; onClose: () => void }> = (
  props,
) => {
  const [drives, setDrives] = createSignal("");
  const [outcome, setOutcome] = createSignal<JoinOutcome>();
  const action = useAction();
  const close = () => {
    setOutcome(undefined);
    setDrives("");
    props.onClose();
  };
  const submit = (e: Event) => {
    e.preventDefault();
    void action.run(async () => {
      setOutcome(await requestNodeJoin(props.api, props.node ?? "", drives()));
    });
  };
  return (
    <Modal
      open={props.node !== undefined}
      title={t("settings:lake.pool.commandFor", { node: props.node ?? "" })}
      onClose={close}
      locked={action.busy() || outcome() !== undefined}
    >
      <Show
        when={outcome()}
        fallback={
          <form class="st-form" onSubmit={submit}>
            <Field
              label={t("settings:machines.join.drives")}
              error={action.fields().drives}
              hint={t("settings:lake.pool.nodeDrivesHint")}
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
            <NoticeBox notice={action.notice()} />
            <div class="st-actions">
              <button type="button" class="fk-btn" disabled={action.busy()} onClick={close}>
                {t("settings:machines.join.cancel")}
              </button>
              <button type="submit" class="fk-btn st-primary" disabled={action.busy()}>
                {action.busy()
                  ? t("settings:machines.join.issuing")
                  : t("settings:machines.join.submit")}
              </button>
            </div>
          </form>
        }
      >
        {(issued) => <JoinResult outcome={issued()} onDone={close} />}
      </Show>
    </Modal>
  );
};

/**
 * The lake's pool `main` (0029 point 5; 0066 step 11): defined here before
 * any of its servers exists, one or more, then each server's command,
 * which names it; once a node has joined, the layout stays.
 */
export const LakePoolCard: Component<{ api: MachinesApi }> = (props) => {
  const [pool, { mutate, refetch }] = createResource(() => loadMainPool(props.api));
  const [asking, setAsking] = createSignal<string>();
  return (
    <div class="card">
      <h2 class="st-subtitle">{t("settings:lake.pool.title")}</h2>
      <p>{t("settings:lake.pool.intro")}</p>
      <Show when={pool.error}>
        <p class="st-notice st-notice--error">
          {pool.error instanceof SettingsApiError
            ? pool.error.message
            : t("settings:lake.pool.loadError")}
        </p>
      </Show>
      <Show when={!definable(pool.latest) && pool.latest}>
        {(p) => (
          <Show when={growsOnlyByCopy(p().nodes.length, p().drives)}>
            <p class="st-notice st-notice--warn">{t("settings:lake.pool.oneDrive")}</p>
          </Show>
        )}
      </Show>
      <Show when={pool.latest}>
        {(p) => (
          <table class="table">
            <thead>
              <tr>
                <th>{t("settings:lake.pool.node")}</th>
                <th>{t("settings:machines.stateLabel")}</th>
                <th>{t("settings:lake.pool.actions")}</th>
              </tr>
            </thead>
            <tbody>
              <For each={p().nodes}>
                {(node) => (
                  <tr>
                    <td class="st-mono">{node.name}</td>
                    <td class="table__muted">{nodeState(node.state)}</td>
                    <td>
                      <button
                        type="button"
                        class="fk-btn fk-btn--sm"
                        onClick={() => setAsking(node.name)}
                      >
                        {t("settings:lake.pool.command")}
                      </button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        )}
      </Show>
      <Show when={!pool.loading && !pool.error && definable(pool.latest)}>
        <PoolForm api={props.api} pool={pool.latest} onDefined={mutate} />
      </Show>
      <NodeCommand
        api={props.api}
        node={asking()}
        onClose={() => {
          setAsking(undefined);
          void refetch();
        }}
      />
    </div>
  );
};
