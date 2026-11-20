import { createMemo, createSignal, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { QualityError, schedulesApi, type ScheduleOut } from "./api";
import { t } from "./i18n";
import { scheduleBody, type ScheduleForm as Form } from "./schedules";
import { describeWhen } from "./trigger";
import { newWhen, problemOf, triggerOf, whenOf, type When } from "./triggerForm";
import { WhenFields, type SetWhen } from "./WhenFields";

const REFUSALS = [
  "fields",
  "cron",
  "trigger",
  "timezone",
  "never_fires",
  "fires",
  "rerun",
  "kind",
  "dataset",
  "unknown_dataset",
];

type Set = <K extends keyof Form>(key: K, value: Form[K]) => void;

/** What a schedule fires: a cut, and what to do if it is there; or a job. */
const FiresFields: Component<{ form: Form; set: Set }> = (props) => (
  <>
    <label class="qd-field">
      <span>{t("quality:schedules.form.fires")}</span>
      <select
        value={props.form.fires}
        onChange={(ev) => props.set("fires", ev.currentTarget.value as Form["fires"])}
      >
        <option value="cut">{t("quality:schedules.fires.cut")}</option>
        <option value="job">{t("quality:schedules.fires.job")}</option>
      </select>
    </label>
    <Show
      when={props.form.fires === "cut"}
      fallback={
        <label class="qd-field">
          <span>{t("quality:schedules.form.kind")}</span>
          <select
            value={props.form.kind}
            onChange={(ev) => props.set("kind", ev.currentTarget.value as Form["kind"])}
          >
            <option value="maintain">{t("quality:schedules.kind.maintain")}</option>
            <option value="map">{t("quality:schedules.kind.map")}</option>
          </select>
        </label>
      }
    >
      <label class="qd-field">
        <span>{t("quality:schedules.form.rerun")}</span>
        <select
          value={props.form.rerun}
          onChange={(ev) => props.set("rerun", ev.currentTarget.value as Form["rerun"])}
        >
          <option value="block">{t("quality:cuts.rerunBlock")}</option>
          <option value="replace">{t("quality:cuts.rerunReplace")}</option>
        </select>
      </label>
    </Show>
  </>
);

/**
 * A schedule's form (decision 0035), as `knime-orchestador`'s: when it
 * fires (once, daily, weekly, monthly from a start, or a cron; a random
 * delay, repetition, expiry), said in words as it is chosen, and what it
 * fires: a dataset's cut (block or replace one that is there) or a job
 * with no cut (map, maintain).
 */
export const ScheduleForm: Component<{
  /** The dataset of the tree's node it is opened from: never chosen here. */
  dataset: string;
  editing?: ScheduleOut;
  onClose: () => void;
  onSaved: () => void;
}> = (props) => {
  const e = props.editing;
  const [when, setWhen] = createSignal<When>(e ? whenOf(e) : newWhen());
  const [form, setForm] = createSignal<Form>({
    cron: "",
    timezone: "",
    enabled: e?.enabled ?? true,
    fires: e?.fires === "job" ? "job" : "cut",
    dataset: props.dataset,
    rerun: e?.rerun === "replace" ? "replace" : "block",
    kind: e?.kind === "map" ? "map" : "maintain",
  });
  const [refusal, setRefusal] = createSignal<string>();
  const set: Set = (key, value) => setForm({ ...form(), [key]: value });
  const setW: SetWhen = (key, value) => setWhen({ ...when(), [key]: value });
  const trigger = createMemo(() => triggerOf(when()));
  const problem = createMemo(() => problemOf(when()));
  const summary = () => problem() ?? describeWhen({ cron: when().cron.trim(), trigger: trigger() });

  const save = async (ev: Event) => {
    ev.preventDefault();
    if (problem()) return;
    setRefusal(undefined);
    try {
      const w = when();
      const body = scheduleBody({
        ...form(),
        cron: w.cron,
        timezone: w.timezone,
        trigger: trigger(),
      });
      await (e ? schedulesApi.update(e.id, body) : schedulesApi.create(body));
      props.onSaved();
    } catch (err) {
      const code =
        err instanceof QualityError ? (err.body as { error?: string })?.error : undefined;
      setRefusal(
        t(`quality:schedules.refusal.${code && REFUSALS.includes(code) ? code : "other"}`),
      );
    }
  };

  return (
    <Modal open title={e ? describeWhen(e) : t("quality:schedules.new")} onClose={props.onClose}>
      <form class="qd-form qd-sched" onSubmit={(ev) => void save(ev)}>
        <WhenFields when={when()} set={setW} />
        <p class="qd-hint">
          {t("quality:schedules.form.dataset")}: <span class="qd-mono">{props.dataset}</span>
        </p>
        <FiresFields form={form()} set={set} />
        <label class="qd-check">
          <input
            type="checkbox"
            checked={form().enabled}
            onChange={(ev) => set("enabled", ev.currentTarget.checked)}
          />
          <span>{t("quality:schedules.form.enabled")}</span>
        </label>
        <Show when={refusal()}>{(r) => <p class="qd-error">{r()}</p>}</Show>
        <div class="qd-actions">
          <span class="qd-sched__summary" classList={{ "qd-sched__summary--bad": !!problem() }}>
            {summary()}
          </span>
          <button type="button" class="fk-btn" onClick={props.onClose}>
            {t("quality:common.cancel")}
          </button>
          <button type="submit" class="fk-btn fk-btn--primary" disabled={!!problem()}>
            {t("quality:common.save")}
          </button>
        </div>
      </form>
    </Modal>
  );
};
