/**
 * Schedule triggers in the style of the Windows Task Scheduler (decision 0035,
 * point 1), as `knime-orchestador`'s form writes them. The same types
 * as the server (`schedule_core::ScheduleTrigger`) and a sentence that
 * describes them, for the form and the list.
 */
import { locale } from "@knime-orchestador/i18n";
import { t } from "./i18n";
import { describeCron } from "./schedules";

export type Recurrence =
  | { kind: "once" }
  | { kind: "daily"; every: number }
  | { kind: "weekly"; every: number; days: number[] }
  | { kind: "monthly"; months: number[]; days: number[]; nth: number[]; weekdays: number[] };

export interface ScheduleTrigger {
  /** `YYYY-MM-DDTHH:MM:SS`, in the schedule's time zone. */
  start: string;
  recurrence: Recurrence;
  repeat?: { every_minutes: number; for_minutes: number | null } | null;
  random_delay_secs: number;
  expires?: string | null;
}

/** A day's name (1 = Monday … 7 = Sunday) in the active language. */
function weekdayName(n: number, width: "short" | "long"): string {
  // 2024-01-01 was a Monday.
  const d = new Date(Date.UTC(2024, 0, n));
  const name = new Intl.DateTimeFormat(locale(), { weekday: width, timeZone: "UTC" }).format(d);
  return width === "long" ? name[0]!.toUpperCase() + name.slice(1) : name.replace(".", "");
}

/** A month's short name (1–12) in the active language. */
function monthName(n: number): string {
  const d = new Date(Date.UTC(2024, n - 1, 1));
  return new Intl.DateTimeFormat(locale(), { month: "short", timeZone: "UTC" })
    .format(d)
    .replace(".", "");
}

/** 1 = Monday … 7 = Sunday, with their names in the active language. */
export const WEEKDAYS = [1, 2, 3, 4, 5, 6, 7].map((value) => ({
  value,
  get short() {
    return weekdayName(value, "short");
  },
  get label() {
    return weekdayName(value, "long");
  },
}));

/** The months (short name) in the active language. */
export function monthNames(): string[] {
  return Array.from({ length: 12 }, (_, i) => monthName(i + 1));
}

/** 1–4, and 5 = the last one. */
export const NTH = [1, 2, 3, 4, 5].map((value) => ({
  value,
  get label() {
    return t(`quality:trigger.nth.${value}`);
  },
}));

/** A day of the month; 32 is «the last». */
export const LAST_DAY = 32;

/** Minutes or seconds in short words: «30 s», «15 min», «2 h», «1 día». */
export function span(secs: number): string {
  if (secs < 60) return `${secs} s`;
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  if (secs < 86400) {
    const h = secs / 3600;
    return `${Number.isInteger(h) ? h : h.toFixed(1)} h`;
  }
  return t("quality:trigger.days", { count: Math.round(secs / 86400) });
}

const list = (items: string[]): string =>
  items.length <= 1
    ? (items[0] ?? "")
    : t("quality:trigger.list", {
        items: items.slice(0, -1).join(", "),
        last: items[items.length - 1],
      });

const sorted = (xs: number[]) => [...xs].sort((a, b) => a - b);

function day(iso: string): string {
  const [y, m, d] = iso.slice(0, 10).split("-").map(Number);
  return t("quality:trigger.date", { day: d, month: monthName(m ?? 1), year: y });
}

const hour = (iso: string) => iso.slice(11, 16);

/** The schedule in one sentence. */
export function describeTrigger(trigger: ScheduleTrigger): string {
  const at = t("quality:trigger.at", { time: hour(trigger.start) });
  const r = trigger.recurrence;
  let what: string;
  switch (r.kind) {
    case "once":
      what = t("quality:trigger.once", { date: day(trigger.start), at });
      break;
    case "daily":
      what = t("quality:trigger.daily", { count: r.every, at });
      break;
    case "weekly": {
      const days = list(sorted(r.days).map((d) => WEEKDAYS[d - 1]?.short ?? "?"));
      what = t("quality:trigger.weekly", { count: r.every, days, at });
      break;
    }
    case "monthly": {
      const months =
        r.months.length === 12
          ? t("quality:trigger.everyMonth")
          : list(sorted(r.months).map((m) => monthName(m)));
      const parts: string[] = [];
      if (r.days.length) {
        const last = r.days.includes(LAST_DAY);
        const ds = sorted(r.days.filter((d) => d !== LAST_DAY)).map(String);
        if (ds.length) parts.push(t("quality:trigger.onDays", { days: list(ds) }));
        if (last) parts.push(t("quality:trigger.lastDay"));
      }
      if (r.nth.length && r.weekdays.length) {
        const which = list(sorted(r.nth).map((n) => (NTH[n - 1]?.label ?? "?").toLowerCase()));
        const days = list(sorted(r.weekdays).map((d) => WEEKDAYS[d - 1]?.short ?? "?"));
        parts.push(t("quality:trigger.nthWeekday", { which, days }));
      }
      what = t("quality:trigger.monthly", { parts: list(parts), months, at });
      what = what[0]!.toUpperCase() + what.slice(1);
      break;
    }
  }
  const extras: string[] = [];
  if (r.kind !== "once") extras.push(t("quality:trigger.since", { date: day(trigger.start) }));
  if (trigger.repeat)
    extras.push(
      trigger.repeat.for_minutes
        ? t("quality:trigger.repeatFor", {
            every: span(trigger.repeat.every_minutes * 60),
            during: span(trigger.repeat.for_minutes * 60),
          })
        : t("quality:trigger.repeatForever", { every: span(trigger.repeat.every_minutes * 60) }),
    );
  if (trigger.random_delay_secs)
    extras.push(t("quality:trigger.randomDelay", { span: span(trigger.random_delay_secs) }));
  if (trigger.expires)
    extras.push(
      t("quality:trigger.expires", { date: day(trigger.expires), time: hour(trigger.expires) }),
    );
  return [what, ...extras].join(" · ");
}

/** When a schedule fires, in words: its trigger's sentence, or its cron's. */
export function describeWhen(s: { cron: string; trigger?: unknown }): string {
  return s.trigger ? describeTrigger(s.trigger as ScheduleTrigger) : describeCron(s.cron);
}
