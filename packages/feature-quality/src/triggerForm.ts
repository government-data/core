/**
 * The _when_ of a schedule's form, as `knime-orchestador`'s form has it
 * (decision 0035, point 1), pure: what is chosen, the trigger it makes (or
 * the cron, in _Custom_), and what is missing to save it.
 */

import { t } from "./i18n";
import { LAST_DAY, type Recurrence, type ScheduleTrigger } from "./trigger";

export type Kind = "once" | "daily" | "weekly" | "monthly" | "custom";

export const KINDS: Kind[] = ["once", "daily", "weekly", "monthly", "custom"];

/** A new schedule's zone until the platform's is known (decision 0079). */
export const DEFAULT_TIMEZONE = "UTC";

/** The most used first; the rest is the browser's IANA list. */
export const COMMON_TIMEZONES = [
  "America/Bogota",
  "America/Lima",
  "America/Mexico_City",
  "America/Santiago",
  "America/Argentina/Buenos_Aires",
  "America/Caracas",
  "America/New_York",
  "Europe/Madrid",
  "UTC",
];

export const RANDOM_DELAYS = [30, 60, 300, 900, 1800, 3600, 8 * 3600, 86400];
export const REPEAT_EVERY = [5, 10, 15, 30, 60];
/** `null`: indefinitely. */
export const REPEAT_FOR: (number | null)[] = [15, 30, 60, 12 * 60, 24 * 60, null];

/** What the form's _when_ holds. */
export interface When {
  kind: Kind;
  startDate: string;
  startTime: string;
  timezone: string;
  everyDays: number;
  everyWeeks: number;
  weekdays: number[];
  months: number[];
  monthMode: "days" | "nth";
  monthDays: number[];
  nth: number[];
  nthWeekdays: number[];
  cron: string;
  delayOn: boolean;
  delay: number;
  repeatOn: boolean;
  repeatEvery: number;
  repeatFor: number | null;
  expiresOn: boolean;
  expiresDate: string;
  expiresTime: string;
}

/** A day as `YYYY-MM-DD`, in the browser's local time. */
const dayOf = (d: Date) => d.toLocaleDateString("en-CA");
const ALL_MONTHS = Array.from({ length: 12 }, (_, i) => i + 1);

/** A new schedule's: daily at 06:00 from today, in `timezone` (the platform's). */
export function newWhen(now = new Date(), timezone = DEFAULT_TIMEZONE): When {
  return {
    kind: "daily",
    startDate: dayOf(now),
    startTime: "06:00",
    timezone,
    everyDays: 1,
    everyWeeks: 1,
    weekdays: [1],
    months: ALL_MONTHS,
    monthMode: "days",
    monthDays: [1],
    nth: [1],
    nthWeekdays: [1],
    cron: "",
    delayOn: false,
    delay: 300,
    repeatOn: false,
    repeatEvery: 15,
    repeatFor: 60,
    expiresOn: false,
    expiresDate: dayOf(new Date(now.getTime() + 365 * 86400000)),
    expiresTime: "23:59",
  };
}

/** A recurrence's fields in the form. */
function recurrenceFields(r: Recurrence): Partial<When> {
  switch (r.kind) {
    case "once":
      return { kind: "once" };
    case "daily":
      return { kind: "daily", everyDays: r.every };
    case "weekly":
      return { kind: "weekly", everyWeeks: r.every, weekdays: r.days };
    case "monthly":
      return r.days.length
        ? { kind: "monthly", months: r.months, monthMode: "days", monthDays: r.days }
        : {
            kind: "monthly",
            months: r.months,
            monthMode: "nth",
            nth: r.nth,
            nthWeekdays: r.weekdays,
          };
  }
}

/** A saved schedule's _when_: its trigger, or its cron as _Custom_. */
export function whenOf(s: { cron: string; timezone: string; trigger?: unknown }): When {
  const base = { ...newWhen(), timezone: s.timezone };
  const tr = s.trigger as ScheduleTrigger | null | undefined;
  if (!tr) return { ...base, kind: "custom", cron: s.cron };
  return {
    ...base,
    ...recurrenceFields(tr.recurrence),
    startDate: tr.start.slice(0, 10),
    startTime: tr.start.slice(11, 16),
    delayOn: tr.random_delay_secs > 0,
    delay: tr.random_delay_secs || base.delay,
    repeatOn: !!tr.repeat,
    repeatEvery: tr.repeat?.every_minutes ?? base.repeatEvery,
    repeatFor: tr.repeat ? (tr.repeat.for_minutes ?? null) : base.repeatFor,
    expiresOn: !!tr.expires,
    expiresDate: tr.expires?.slice(0, 10) ?? base.expiresDate,
    expiresTime: tr.expires?.slice(11, 16) ?? base.expiresTime,
  };
}

function recurrence(w: When): Recurrence | null {
  switch (w.kind) {
    case "once":
      return { kind: "once" };
    case "daily":
      return { kind: "daily", every: w.everyDays };
    case "weekly":
      return { kind: "weekly", every: w.everyWeeks, days: w.weekdays };
    case "monthly":
      return w.monthMode === "days"
        ? { kind: "monthly", months: w.months, days: w.monthDays, nth: [], weekdays: [] }
        : { kind: "monthly", months: w.months, days: [], nth: w.nth, weekdays: w.nthWeekdays };
    default:
      return null;
  }
}

/** The trigger the form makes; none in _Custom_, which is a cron. */
export function triggerOf(w: When): ScheduleTrigger | null {
  const r = recurrence(w);
  if (!r) return null;
  return {
    start: `${w.startDate}T${w.startTime}:00`,
    recurrence: r,
    repeat: w.repeatOn ? { every_minutes: w.repeatEvery, for_minutes: w.repeatFor } : null,
    random_delay_secs: w.delayOn ? w.delay : 0,
    expires: w.expiresOn ? `${w.expiresDate}T${w.expiresTime}:00` : null,
  };
}

/** The key of what a recurrence lacks, or `null`. */
function recurrenceProblem(w: When): string | null {
  if (w.kind === "daily" && !(w.everyDays >= 1 && w.everyDays <= 365)) return "everyDays";
  if (w.kind === "weekly") {
    if (!(w.everyWeeks >= 1 && w.everyWeeks <= 52)) return "everyWeeks";
    if (!w.weekdays.length) return "weekdays";
  }
  if (w.kind === "monthly") {
    if (!w.months.length) return "months";
    if (w.monthMode === "days" && !w.monthDays.length) return "monthDays";
    if (w.monthMode === "nth" && (!w.nth.length || !w.nthWeekdays.length)) return "nth";
  }
  return null;
}

/** What is missing to save, in words, or `null`. */
export function problemOf(w: When): string | null {
  const key = (() => {
    if (w.kind === "custom") return w.cron.trim() ? null : "cron";
    if (!w.startDate || !w.startTime) return "start";
    const missing = recurrenceProblem(w);
    if (missing) return missing;
    if (w.repeatOn && w.repeatFor !== null && w.repeatFor < w.repeatEvery) return "repeat";
    if (w.expiresOn && `${w.expiresDate}T${w.expiresTime}` <= `${w.startDate}T${w.startTime}`)
      return "expires";
    return null;
  })();
  return key ? t(`quality:schedules.whenForm.problem.${key}`) : null;
}

/** The days of a month to choose from: 1–31 and the last. */
export const MONTH_DAYS = [...Array.from({ length: 31 }, (_, i) => i + 1), LAST_DAY];
