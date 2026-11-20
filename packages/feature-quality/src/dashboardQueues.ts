/**
 * The remediation queues as a figure (decision 0059), one per dimension:
 * each queue's open findings and those overdue by its SLA, with what was
 * resolved at the cut, the oldest open finding's age and the summed
 * priority in its table. Pure, as `dashboard.ts`.
 */

import { columns, num, type Figure, type Lang } from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));
const group = (tr: Texts, v: unknown) =>
  v === null || v === undefined || v === "" ? tr("emptyGroup") : String(v);

const OPEN = "var(--viz-s1)";
const OVERDUE = "var(--viz-crit)";

export function queues(lang: Lang, rows: Row[], tr: Texts): Figure {
  const cats = rows.map((r) => group(tr, r.grp));
  const open = rows.map((r) => n(r.open));
  const overdue = rows.map((r) => n(r.overdue));
  const days = (v: unknown) =>
    v === null || v === undefined ? "—" : `${num(lang, n(v))} ${tr("days")}`;
  return {
    title: tr("remediation_queues"),
    note: tr("queuesNote"),
    live: true,
    plot: {
      svg: columns({
        cats,
        series: [
          { label: tr("openFindings"), color: OPEN, values: open },
          { label: tr("overdueFindings"), color: OVERDUE, values: overdue },
        ],
        yMax: Math.max(1, ...open),
        yFmt: (v) => num(lang, v),
        valFmt: (v) => num(lang, v),
      }),
      legend: [
        { label: tr("openFindings"), color: OPEN },
        { label: tr("overdueFindings"), color: OVERDUE },
      ],
      table: {
        head: [
          tr("group"),
          tr("openFindings"),
          tr("overdueFindings"),
          tr("resolvedFindings"),
          tr("oldestOpen"),
          tr("sla"),
          tr("priority"),
        ],
        rows: rows.map((r, i) => [
          cats[i],
          num(lang, open[i]),
          num(lang, overdue[i]),
          num(lang, n(r.resolved)),
          days(r.oldest_days),
          days(r.sla_days),
          num(lang, n(r.priority), 2),
        ]),
      },
    },
  };
}
