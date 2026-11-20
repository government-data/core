import { createResource, createSignal, Show, type Component } from "solid-js";
import { ConfirmDialog, Status } from "@knime-orchestador/feature-kit/ui";
import { formatDateTime } from "@knime-orchestador/i18n";
import { may, QualityError, schedulesApi, type ScheduleOut } from "./api";
import { schedulesChanged, who } from "./explorerCache";
import { t } from "./i18n";
import { notify, warn } from "./Notify";
import { ScheduleForm } from "./ScheduleForm";
import { outcome } from "./schedules";
import { describeWhen } from "./trigger";

/**
 * _×_ beside a schedule of the tree, for whoever may manage it: asks
 * first, then deletes it; one that has fired cuts is refused by the API
 * (disable it instead), and that is said in a toast.
 */
const RemoveSchedule: Component<{ schedule: ScheduleOut }> = (props) => {
  const [asking, setAsking] = createSignal(false);
  const what = () => describeWhen(props.schedule);
  const remove = async () => {
    setAsking(false);
    try {
      await schedulesApi.remove(props.schedule.id);
      notify(t("quality:schedules.deleted"));
      schedulesChanged();
    } catch (err) {
      const hasCuts = err instanceof QualityError && err.status === 409;
      if (hasCuts) warn(t("quality:schedules.hasCuts"));
      else notify(t("quality:common.saveError"), true);
    }
  };
  return (
    <>
      <button
        type="button"
        class="qd-tree-add"
        title={t("quality:explorer.removeSchedule", { what: what() })}
        aria-label={t("quality:explorer.removeSchedule", { what: what() })}
        onClick={(e) => {
          e.preventDefault();
          setAsking(true);
        }}
      >
        ×
      </button>
      <ConfirmDialog
        open={asking()}
        title={what()}
        message={t("quality:schedules.confirmDelete")}
        onCancel={() => setAsking(false)}
        onConfirm={() => void remove()}
      />
    </>
  );
};

/** A schedule in a line: when (which opens it to edit, for whoever may), paused or its next run, its last outcome, and _×_. */
export const ScheduleItem: Component<{ schedule: ScheduleOut }> = (props) => {
  const [me] = createResource(who);
  const [editing, setEditing] = createSignal(false);
  const manages = () => may(me(), "quality:ManageSchedules");
  return (
    <li role="treeitem" class="qd-tree-leaf">
      <Show when={manages()} fallback={<span>{describeWhen(props.schedule)}</span>}>
        <button
          type="button"
          class="qd-link qd-tree-goto"
          title={t("quality:explorer.editSchedule")}
          onClick={() => setEditing(true)}
        >
          {describeWhen(props.schedule)}
        </button>
      </Show>{" "}
      <Show
        when={props.schedule.enabled}
        fallback={<Status tone="neutral" label={t("quality:schedules.disabled")} />}
      >
        <Show when={props.schedule.next_fire_at}>
          {(at) => (
            <small class="qd-hint">
              {t("quality:explorer.next", { at: formatDateTime(at()) })}
            </small>
          )}
        </Show>{" "}
        <Show when={props.schedule.last_outcome}>
          <Status {...outcome(props.schedule.last_outcome)} />
        </Show>
      </Show>{" "}
      <Show when={manages()}>
        <RemoveSchedule schedule={props.schedule} />
      </Show>
      <Show when={editing()}>
        <ScheduleForm
          dataset={props.schedule.dataset ?? ""}
          editing={props.schedule}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false);
            schedulesChanged();
          }}
        />
      </Show>
    </li>
  );
};

/**
 * _+_ beside a dataset's schedules, for whoever may manage them: the
 * schedule's form over the tree, for that dataset.
 */
export const AddSchedule: Component<{ dataset: string }> = (props) => {
  const [me] = createResource(who);
  const [open, setOpen] = createSignal(false);
  return (
    <Show when={may(me(), "quality:ManageSchedules")}>
      <button
        type="button"
        class="qd-tree-add"
        title={t("quality:explorer.addSchedule", { dataset: props.dataset })}
        aria-label={t("quality:explorer.addSchedule", { dataset: props.dataset })}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setOpen(true);
        }}
      >
        +
      </button>
      <Show when={open()}>
        <ScheduleForm
          dataset={props.dataset}
          onClose={() => setOpen(false)}
          onSaved={() => {
            setOpen(false);
            schedulesChanged();
          }}
        />
      </Show>
    </Show>
  );
};
