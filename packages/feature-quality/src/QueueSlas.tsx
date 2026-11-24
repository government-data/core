import {
  createEffect,
  createResource,
  createSignal,
  Index,
  Show,
  type Accessor,
  type Component,
} from "solid-js";
import { QualityError } from "./apiCore";
import { slasApi, type SlasOut } from "./dashboardApi";
import { t } from "./i18n";
import { LoadFailed, notify, warn } from "./Notify";
import { formOf, slasOf, type DimensionSlas, type Override } from "./queueSlas";

/** Bumped when a dataset's SLAs are saved: the queues' chart reads again. */
export const [slasVersion, setSlasVersion] = createSignal(0);

const REFUSALS = ["dimension", "days", "group", "duplicate", "too_many", "not_found"];

/** What a refused save says: the API's code when it has a text, else a failure. */
function refused(err: unknown) {
  const code =
    err instanceof QualityError ? (err.body as { error?: string } | undefined)?.error : undefined;
  if (err instanceof QualityError && err.status === 403) warn(t("quality:common.forbidden"));
  else if (code === "catalog_not_configured") warn(t("quality:common.noCatalog"));
  else if (code === "catalog_unavailable") warn(t("quality:common.catalogUnavailable"));
  else if (code && REFUSALS.includes(code)) warn(t(`quality:slas.refusal.${code}`));
  else notify(t("quality:common.saveError"), true);
}

type Edit = (change: (d: DimensionSlas) => DimensionSlas) => void;

/** A group's override: its value and its days, and × to drop it. */
const OverrideRow: Component<{ o: Accessor<Override>; set: (o: Override | null) => void }> = (
  props,
) => (
  <div class="qd-slas-row">
    <label class="qd-field">
      <span>{t("quality:slas.group")}</span>
      <input
        value={props.o().group}
        placeholder={t("quality:charts.emptyGroup")}
        onInput={(e) => props.set({ ...props.o(), group: e.currentTarget.value })}
      />
    </label>
    <label class="qd-field">
      <span>{t("quality:slas.days")}</span>
      <input
        inputmode="numeric"
        value={props.o().days}
        onInput={(e) => props.set({ ...props.o(), days: e.currentTarget.value })}
      />
    </label>
    <button
      type="button"
      class="fk-btn"
      aria-label={t("quality:slas.removeGroup")}
      title={t("quality:slas.removeGroup")}
      onClick={() => props.set(null)}
    >
      ×
    </button>
  </div>
);

/** A dimension's default days and its groups' overrides. */
const DimensionFields: Component<{ d: Accessor<DimensionSlas>; edit: Edit; fallback: number }> = (
  props,
) => {
  const setOverride = (j: number, o: Override | null) =>
    props.edit((d) => ({
      ...d,
      overrides: o
        ? d.overrides.map((x, k) => (k === j ? o : x))
        : d.overrides.filter((_, k) => k !== j),
    }));
  return (
    <fieldset class="qd-slas-dimension">
      <legend>{props.d().dimension}</legend>
      <label class="qd-field">
        <span>{t("quality:slas.default")}</span>
        <input
          inputmode="numeric"
          value={props.d().days}
          placeholder={String(props.fallback)}
          onInput={(e) => props.edit((d) => ({ ...d, days: e.currentTarget.value }))}
        />
      </label>
      <Index each={props.d().overrides}>
        {(o, j) => <OverrideRow o={o} set={(x) => setOverride(j, x)} />}
      </Index>
      <button
        type="button"
        class="fk-btn"
        onClick={() =>
          props.edit((d) => ({ ...d, overrides: [...d.overrides, { group: "", days: "" }] }))
        }
      >
        {t("quality:slas.addGroup")}
      </button>
    </fieldset>
  );
};

/**
 * The remediation queues' SLAs of a dataset (decision 0059), for whoever
 * may edit its pipeline: per dimension its default days and the groups
 * overridden. Saving replaces the dataset's set and redraws the queues.
 */
export const QueueSlas: Component<{ dataset: string }> = (props) => {
  const [read] = createResource(() => props.dataset, slasApi.read);
  const [form, setForm] = createSignal<DimensionSlas[]>([]);
  const [busy, setBusy] = createSignal(false);
  const fill = (out: SlasOut) => setForm(formOf(out.dimensions, out.slas));
  createEffect(() => {
    const out = read();
    if (out) fill(out);
  });
  const save = async () => {
    const body = slasOf(form());
    if ("problem" in body) {
      warn(t(`quality:slas.refusal.${body.problem}`));
      return;
    }
    setBusy(true);
    try {
      fill(await slasApi.replace(props.dataset, body));
      setSlasVersion((v) => v + 1);
      notify(t("quality:slas.saved"));
    } catch (err) {
      refused(err);
    } finally {
      setBusy(false);
    }
  };
  return (
    <details class="qd-slas">
      <summary>{t("quality:slas.title")}</summary>
      <LoadFailed when={read.error} />
      <Show when={read()}>
        {(out) => (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <p class="qd-hint">{t("quality:slas.note", { days: out().default_days })}</p>
            <Show when={form().length} fallback={<p class="qd-hint">{t("quality:slas.none")}</p>}>
              <Index each={form()}>
                {(d, i) => (
                  <DimensionFields
                    d={d}
                    fallback={out().default_days}
                    edit={(change) => setForm((f) => f.map((x, k) => (k === i ? change(x) : x)))}
                  />
                )}
              </Index>
              <button type="submit" class="fk-btn fk-btn--primary" disabled={busy()}>
                {t("quality:common.save")}
              </button>
            </Show>
          </form>
        )}
      </Show>
    </details>
  );
};
