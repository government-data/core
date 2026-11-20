import { For, Match, Show, Switch, type Component } from "solid-js";
import { t } from "./i18n";
import { monthNames, NTH, WEEKDAYS } from "./trigger";
import { Advanced } from "./WhenAdvanced";
import { COMMON_TIMEZONES, KINDS, MONTH_DAYS, type When } from "./triggerForm";

export type SetWhen = <K extends keyof When>(key: K, value: When[K]) => void;
export type WhenProps = { when: When; set: SetWhen };
type Props = WhenProps;

const otherTimezones = (): string[] => {
  try {
    return Intl.supportedValuesOf("timeZone").filter((tz) => !COMMON_TIMEZONES.includes(tz));
  } catch {
    return [];
  }
};

/** Boxes to tick some of `items`: days, months, weeks of the month. */
const Checks: Component<{
  items: { value: number; label: string }[];
  chosen: number[];
  onChange: (v: number[]) => void;
  label: string;
}> = (props) => (
  <div class="qd-sched__checks" role="group" aria-label={props.label}>
    <For each={props.items}>
      {(it) => (
        <label class="qd-check">
          <input
            type="checkbox"
            checked={props.chosen.includes(it.value)}
            onChange={() =>
              props.onChange(
                props.chosen.includes(it.value)
                  ? props.chosen.filter((x) => x !== it.value)
                  : [...props.chosen, it.value],
              )
            }
          />
          {it.label}
        </label>
      )}
    </For>
  </div>
);

const weekdayItems = () => WEEKDAYS.map((d) => ({ value: d.value, label: d.label }));
const w = (key: string) => t(`quality:schedules.whenForm.${key}`);

/** Every N days or weeks: a number between two words. */
const Every: Component<{
  value: number;
  max: number;
  after: string;
  onChange: (n: number) => void;
}> = (props) => (
  <label class="qd-sched__inline">
    {w("repeatEvery")}
    <input
      type="number"
      min="1"
      max={props.max}
      value={props.value}
      onInput={(e) => props.onChange(e.currentTarget.valueAsNumber)}
    />
    {props.after}
  </label>
);

/** A month's: which months, then some days of them or the nth weekday. */
const Monthly: Component<Props> = (props) => (
  <>
    <div class="qd-sched__months">
      <span class="qd-hint">{w("months")}</span>
      <button
        type="button"
        class="fk-btn fk-btn--sm"
        onClick={() =>
          props.set(
            "months",
            props.when.months.length === 12 ? [] : monthNames().map((_, i) => i + 1),
          )
        }
      >
        {props.when.months.length === 12 ? w("none") : w("every")}
      </button>
    </div>
    <Checks
      label={w("months")}
      items={monthNames().map((m, i) => ({ value: i + 1, label: m }))}
      chosen={props.when.months}
      onChange={(v) => props.set("months", v)}
    />
    <For each={["days", "nth"] as const}>
      {(mode) => (
        <>
          <label class="qd-check">
            <input
              type="radio"
              name="month-mode"
              checked={props.when.monthMode === mode}
              onChange={() => props.set("monthMode", mode)}
            />
            {w(mode === "days" ? "daysMode" : "onMode")}
          </label>
          <Show when={props.when.monthMode === mode && mode === "days"}>
            <Checks
              label={w("monthDays")}
              items={MONTH_DAYS.map((d) => ({ value: d, label: d > 31 ? w("last") : String(d) }))}
              chosen={props.when.monthDays}
              onChange={(v) => props.set("monthDays", v)}
            />
          </Show>
          <Show when={props.when.monthMode === mode && mode === "nth"}>
            <Checks
              label={w("weekOfMonth")}
              items={NTH}
              chosen={props.when.nth}
              onChange={(v) => props.set("nth", v)}
            />
            <Checks
              label={w("weekday")}
              items={weekdayItems()}
              chosen={props.when.nthWeekdays}
              onChange={(v) => props.set("nthWeekdays", v)}
            />
          </Show>
        </>
      )}
    </For>
  </>
);

/** The start (or the cron, in _Custom_), the timezone, and how often. */
const Detail: Component<Props> = (props) => (
  <div class="qd-sched__detail">
    <Show
      when={props.when.kind !== "custom"}
      fallback={
        <label class="qd-field">
          <span>{w("cron")}</span>
          <input
            class="qd-mono"
            placeholder="0 6 * * 1-5"
            value={props.when.cron}
            onInput={(e) => props.set("cron", e.currentTarget.value)}
          />
        </label>
      }
    >
      <div class="qd-sched__row">
        <label class="qd-field">
          <span>{w("start")}</span>
          <input
            type="date"
            value={props.when.startDate}
            onInput={(e) => props.set("startDate", e.currentTarget.value)}
          />
        </label>
        <label class="qd-field">
          <span>{w("time")}</span>
          <input
            type="time"
            value={props.when.startTime}
            onInput={(e) => props.set("startTime", e.currentTarget.value)}
          />
        </label>
      </div>
    </Show>
    <label class="qd-field">
      <span>{t("quality:schedules.form.timezone")}</span>
      <select
        value={props.when.timezone}
        onChange={(e) => props.set("timezone", e.currentTarget.value)}
      >
        <optgroup label={w("common")}>
          <For each={COMMON_TIMEZONES}>{(tz) => <option value={tz}>{tz}</option>}</For>
        </optgroup>
        <optgroup label={w("all")}>
          <For each={otherTimezones()}>{(tz) => <option value={tz}>{tz}</option>}</For>
        </optgroup>
      </select>
    </label>
    <Switch>
      <Match when={props.when.kind === "daily"}>
        <Every
          value={props.when.everyDays}
          max={365}
          after={w("days")}
          onChange={(n) => props.set("everyDays", n)}
        />
      </Match>
      <Match when={props.when.kind === "weekly"}>
        <Every
          value={props.when.everyWeeks}
          max={52}
          after={w("weeksOn")}
          onChange={(n) => props.set("everyWeeks", n)}
        />
        <Checks
          label={w("weekdays")}
          items={weekdayItems()}
          chosen={props.when.weekdays}
          onChange={(v) => props.set("weekdays", v)}
        />
      </Match>
      <Match when={props.when.kind === "monthly"}>
        <Monthly when={props.when} set={props.set} />
      </Match>
    </Switch>
  </div>
);

/**
 * When a schedule fires, as `knime-orchestador`'s form asks it (decision
 * 0035, point 1): once, daily, weekly or monthly from a start, or a cron;
 * then a random delay, repetition and expiry.
 */
export const WhenFields: Component<Props> = (props) => (
  <>
    <div class="qd-sched__main">
      <div class="qd-sched__kinds" role="radiogroup" aria-label={w("frequency")}>
        <For each={KINDS}>
          {(k) => (
            <label class="qd-check">
              <input
                type="radio"
                name="frequency"
                checked={props.when.kind === k}
                onChange={() => props.set("kind", k)}
              />
              {t(`quality:schedules.whenForm.kinds.${k}`)}
            </label>
          )}
        </For>
      </div>
      <Detail when={props.when} set={props.set} />
    </div>
    <Show when={props.when.kind !== "custom"}>
      <Advanced when={props.when} set={props.set} />
    </Show>
  </>
);
