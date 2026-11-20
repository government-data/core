/**
 * The remediation queues' SLAs as a form edits them (decision 0059): per
 * dimension its default days and the groups overridden, read from the
 * API's set and sent back whole (the API replaces the dataset's set). Days
 * are typed as text; a blank default leaves the API's (30 days). Pure.
 */

/** One SLA as the API has it: `group` null for the dimension's default. */
export interface Sla {
  dimension: string;
  group?: string | null;
  days: number;
}

/** A group's override in the form. */
export interface Override {
  group: string;
  days: string;
}

/** A dimension's SLAs in the form. */
export interface DimensionSlas {
  dimension: string;
  /** Blank: none set, the API's default holds. */
  days: string;
  overrides: Override[];
}

/** The longest SLA the API takes, as `sla_api::MAX_DAYS`. */
export const MAX_DAYS = 365;

/** The form for `dimensions`, filled with the SLAs set on them. */
export function formOf(dimensions: string[], slas: Sla[]): DimensionSlas[] {
  return dimensions.map((dimension) => {
    const own = slas.filter((s) => s.dimension === dimension);
    const fallback = own.find((s) => s.group === null || s.group === undefined);
    return {
      dimension,
      days: fallback ? String(fallback.days) : "",
      overrides: own
        .filter((s) => typeof s.group === "string")
        .map((s) => ({ group: s.group as string, days: String(s.days) })),
    };
  });
}

/** Days typed, if a whole number from 1 to {@link MAX_DAYS}. */
export function daysOf(text: string): number | undefined {
  const t = text.trim();
  if (!/^\d+$/.test(t)) return undefined;
  const n = Number(t);
  return n >= 1 && n <= MAX_DAYS ? n : undefined;
}

/** Why the form cannot be sent, as the API's refusal codes. */
export type Problem = "days" | "duplicate";

/** The API's body for the form, or the first problem it has. */
export function slasOf(form: DimensionSlas[]): { slas: Sla[] } | { problem: Problem } {
  const slas: Sla[] = [];
  for (const d of form) {
    if (d.days.trim() !== "") {
      const days = daysOf(d.days);
      if (days === undefined) return { problem: "days" };
      slas.push({ dimension: d.dimension, group: null, days });
    }
    const seen = new Set<string>();
    for (const o of d.overrides) {
      const days = daysOf(o.days);
      if (days === undefined) return { problem: "days" };
      if (seen.has(o.group)) return { problem: "duplicate" };
      seen.add(o.group);
      slas.push({ dimension: d.dimension, group: o.group, days });
    }
  }
  return { slas };
}
