/**
 * What the cuts and schedules screen computes (decision 0035), pure: a
 * cron in words for the common cases, the tone and label of a cut's state
 * and of a schedule's last outcome (0040: never color alone), and a
 * schedule's form as the API's body.
 */

import type { Tone } from "@knime-orchestador/feature-kit/ui";
import type { ScheduleBody } from "./api";
import { t } from "./i18n";
import type { ScheduleTrigger } from "./trigger";

const pad = (n: string) => n.padStart(2, "0");
const NUMBER = /^\d{1,2}$/;

/** A five-field cron in words when it is a common one; else as written. */
export function describeCron(expr: string): string {
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5) return expr;
  const [minute, hour, day, month, weekday] = parts as [string, string, string, string, string];
  if (!NUMBER.test(minute) || !NUMBER.test(hour) || month !== "*") return expr;
  const time = `${pad(hour)}:${pad(minute)}`;
  if (day === "*" && weekday === "*") return t("quality:cron.daily", { time });
  if (day === "*" && weekday === "1-5") return t("quality:cron.weekdays", { time });
  if (day === "*" && /^[0-6]$/.test(weekday)) {
    return t("quality:cron.weekly", { time, day: t(`quality:cron.day.${weekday}`) });
  }
  if (NUMBER.test(day) && weekday === "*") return t("quality:cron.monthly", { time, day });
  return expr;
}

/** A cut's state as a tone and a label. */
export function cutState(state: string): { tone: Tone; label: string } {
  const tones: Record<string, Tone> = {
    done: "good",
    evaluating: "warning",
    extracting: "warning",
    failed: "critical",
  };
  const known = state in tones;
  return {
    tone: tones[state] ?? "neutral",
    label: t(`quality:cuts.state.${known ? state : "other"}`),
  };
}

/** A schedule's last outcome as a tone and a label. */
export function outcome(last: string | null | undefined): { tone: Tone; label: string } {
  const tones: Record<string, Tone> = {
    queued: "good",
    replaced: "good",
    blocked: "serious",
    skipped: "warning",
  };
  if (!last || !(last in tones))
    return { tone: "neutral", label: t("quality:schedules.outcome.none") };
  return { tone: tones[last] ?? "neutral", label: t(`quality:schedules.outcome.${last}`) };
}

/** A schedule's form. */
export interface ScheduleForm {
  cron: string;
  /** When set, it fires by it and the cron is not sent. */
  trigger?: ScheduleTrigger | null;
  timezone: string;
  enabled: boolean;
  fires: "cut" | "job";
  dataset: string;
  rerun: "block" | "replace";
  kind: "map" | "maintain";
}

/** The form as the API's body: a cut's fields or a job's, never both; a trigger or a cron. */
export function scheduleBody(f: ScheduleForm): ScheduleBody {
  const base = {
    ...(f.trigger
      ? { cron: "", trigger: f.trigger as unknown as ScheduleBody["trigger"] }
      : { cron: f.cron.trim() }),
    timezone: f.timezone.trim(),
    enabled: f.enabled,
    dataset: f.dataset,
  };
  return f.fires === "cut"
    ? { ...base, fires: "cut", rerun: f.rerun }
    : { ...base, fires: "job", kind: f.kind };
}
