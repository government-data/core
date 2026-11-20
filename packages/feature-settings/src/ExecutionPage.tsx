import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { ConfirmDialog } from "@knime-orchestador/feature-kit/ui";
import { may, meApi, SettingsApiError } from "./api";
import { NoticeBox, Panel, useAction } from "./common";
import { t } from "./i18n";
import { JoinMachine } from "./JoinMachine";
import { canAddMachine } from "./joinRequest";
import {
  actOn,
  machineActions,
  machinesApi,
  type Machine,
  type MachineAction,
} from "./machinesApi";

const MANAGE = "admin:ManageMachines";

/** An epoch in seconds as the person's date and time, or a dash. */
const dateTime = (epoch?: number | null) => (epoch ? new Date(epoch * 1000).toLocaleString() : "—");

/** A machine's state as a badge: pending stands out, the others are muted. */
const StateBadge: Component<{ state: string }> = (props) => (
  <span
    class="badge"
    classList={{ "badge--muted": props.state !== "pending" && props.state !== "approved" }}
  >
    {t(`settings:machines.state.${props.state}`)}
  </span>
);

/** One machine's row, with what the person may do to it. */
const MachineRow: Component<{
  machine: Machine;
  manages: boolean;
  busy: boolean;
  onAction: (machine: Machine, action: MachineAction) => void;
}> = (props) => (
  <tr>
    <td>
      <div class="st-mono">{props.machine.name}</div>
      <div class="table__muted">
        {t("settings:machines.joined", { at: dateTime(props.machine.created_at) })}
      </div>
    </td>
    <td>{props.machine.role}</td>
    <td>{props.machine.pool ?? "—"}</td>
    <td>
      <StateBadge state={props.machine.state} />
    </td>
    <td class="table__muted">{props.machine.version ?? "—"}</td>
    <td class="table__muted">{dateTime(props.machine.cert_not_after)}</td>
    <td class="st-linked__actions">
      <For each={machineActions(props.machine.state, props.manages)}>
        {(action) => (
          <>
            <button
              type="button"
              class="fk-btn fk-btn--sm"
              classList={{
                "st-primary": action === "approve",
                "fk-btn--danger": action === "retire",
              }}
              disabled={props.busy}
              onClick={() => props.onAction(props.machine, action)}
            >
              {t(`settings:machines.action.${action}`)}
            </button>{" "}
          </>
        )}
      </For>
    </td>
  </tr>
);

/** The machines' table, or a line when there are none. */
const MachinesTable: Component<{
  machines: Machine[];
  manages: boolean;
  busy: boolean;
  onAction: (machine: Machine, action: MachineAction) => void;
}> = (props) => (
  <div class="card">
    <Show
      when={props.machines.length > 0}
      fallback={<p class="table__muted">{t("settings:machines.empty")}</p>}
    >
      <table class="table">
        <thead>
          <tr>
            <th>{t("settings:machines.name")}</th>
            <th>{t("settings:machines.role")}</th>
            <th>{t("settings:machines.pool")}</th>
            <th>{t("settings:machines.stateLabel")}</th>
            <th>{t("settings:machines.version")}</th>
            <th>{t("settings:machines.certificate")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          <For each={props.machines}>
            {(machine) => (
              <MachineRow
                machine={machine}
                manages={props.manages}
                busy={props.busy}
                onAction={props.onAction}
              />
            )}
          </For>
        </tbody>
      </table>
    </Show>
  </div>
);

/**
 * Settings → Execution (decisions 0029 and 0066): the installation's
 * machines. A new one joins pending and works only once an administrator
 * approves it here, the one act of a person in its certificate's life; an
 * approved one can be drained, and any retired, which is asked first. A new
 * one is added with the command shown once by "Add a machine".
 */
export const ExecutionPage: Component = () => {
  const [me] = createResource(meApi);
  const [list, { refetch }] = createResource(machinesApi.list);
  const [retiring, setRetiring] = createSignal<Machine>();
  const row = useAction();
  const manages = () => may(me(), MANAGE);

  const run = (machine: Machine, action: MachineAction) =>
    row.run(() => actOn(machinesApi, machine, action, refetch));

  const onAction = (machine: Machine, action: MachineAction) => {
    if (action === "retire") setRetiring(machine);
    else void run(machine, action);
  };

  const retire = () => {
    const machine = retiring();
    if (machine) void run(machine, "retire").finally(() => setRetiring(undefined));
  };

  return (
    <Panel title={t("settings:machines.title")} subtitle={t("settings:machines.subtitle")}>
      <Show when={list.error}>
        <p class="st-notice st-notice--error">
          {list.error instanceof SettingsApiError
            ? list.error.message
            : t("settings:machines.loadError")}
        </p>
      </Show>
      <Show when={list.loading && !list.latest}>
        <p class="table__muted">{t("settings:machines.loading")}</p>
      </Show>
      <NoticeBox notice={row.notice()} />
      <Show when={list.latest}>
        {(data) => (
          <>
            <Show when={!data().can_join}>
              <p class="st-notice st-notice--warn">{t("settings:machines.noCa")}</p>
            </Show>
            <Show when={canAddMachine(manages(), data().can_join)}>
              <JoinMachine onClosed={() => void refetch()} />
            </Show>
            <MachinesTable
              machines={data().machines}
              manages={manages()}
              busy={row.busy()}
              onAction={onAction}
            />
          </>
        )}
      </Show>
      <ConfirmDialog
        open={retiring() !== undefined}
        title={t("settings:machines.confirmTitle")}
        message={t("settings:machines.confirmBody", { name: retiring()?.name ?? "" })}
        confirmLabel={t("settings:machines.action.retire")}
        pending={row.busy()}
        onCancel={() => setRetiring(undefined)}
        onConfirm={retire}
      />
    </Panel>
  );
};
