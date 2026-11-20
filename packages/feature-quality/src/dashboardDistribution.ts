/**
 * The distributions' charts as figures (decision 0031, the guide's
 * formulas 7 and 8): the outliers of an amount by group, and the PSI of
 * each drift variable by the previous cut's deciles. Pure, as
 * `dashboard.ts`.
 */

import { columns, num, pct, stacked, type Figure, type Lang } from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));
const group = (tr: Texts, v: unknown) =>
  v === null || v === undefined || v === "" ? tr("emptyGroup") : String(v);

/** Stable below 0.10, moderate drift to 0.25, significant above. */
export function psiReading(v: number): "stable" | "moderate" | "significant" {
  return v < 0.1 ? "stable" : v <= 0.25 ? "moderate" : "significant";
}

export function outliers(lang: Lang, rows: Row[], tr: Texts): Figure {
  // Only the groups with an outlier are drawn; the table has every one.
  const drawn = rows.filter((r) => n(r.low) + n(r.high) > 0);
  const share = (r: Row) => pct(lang, n(r.share), 1);
  return {
    title: tr("outliers_by_group"),
    note: `${String(rows[0]?.variable ?? "")} · ${tr(drawn.length ? "outliersNote" : "noOutliers")}`,
    live: true,
    plot: {
      svg: stacked({
        rows: drawn.map((r) => ({ label: group(tr, r.grp), parts: [n(r.low), n(r.high)] })),
        series: [
          { label: tr("outliersLow"), color: "var(--viz-s2)", ink: "#ffffff" },
          { label: tr("outliersHigh"), color: "var(--viz-s1)", ink: "#ffffff" },
        ],
        percent: false,
        max: Math.max(1, ...rows.map((r) => n(r.low) + n(r.high))),
        fmt: (v) => num(lang, v),
        totalFmt: (v) => num(lang, v),
      }),
      legend: [
        { label: tr("outliersLow"), color: "var(--viz-s2)" },
        { label: tr("outliersHigh"), color: "var(--viz-s1)" },
      ],
      table: {
        head: [tr("group"), tr("amounts"), tr("outliersLow"), tr("outliersHigh"), tr("share")],
        rows: rows.map((r) => [
          group(tr, r.grp),
          num(lang, n(r.records)),
          num(lang, n(r.low)),
          num(lang, n(r.high)),
          share(r),
        ]),
      },
    },
  };
}

export function psi(lang: Lang, rows: Row[], tr: Texts): Figure {
  const value = n(rows[0]?.psi);
  const cats = rows.map((r) => String(r.decile));
  return {
    title: tr("psi_by_decile"),
    note: `PSI = ${num(lang, value, 3)} · ${tr(`psiReading.${psiReading(value)}`)}`,
    live: true,
    plot: {
      svg: columns({
        cats,
        series: [
          {
            label: tr("previousCut"),
            color: "var(--viz-muted)",
            values: rows.map((r) => n(r.base)),
          },
          { label: tr("thisCut"), color: "var(--viz-s1)", values: rows.map((r) => n(r.now)) },
        ],
        // Up to the next tenth, so the tallest column ends on a grid line.
        yMax: Math.min(
          1,
          Math.ceil(Math.max(0.2, ...rows.map((r) => Math.max(n(r.base), n(r.now)))) * 10) / 10,
        ),
        yFmt: (v) => pct(lang, v),
        valFmt: (v) => pct(lang, v, 1),
      }),
      legend: [
        { label: tr("previousCut"), color: "var(--viz-muted)" },
        { label: tr("thisCut"), color: "var(--viz-s1)" },
      ],
      table: {
        head: [tr("decile"), tr("previousCut"), tr("thisCut")],
        rows: rows.map((r) => [String(r.decile), pct(lang, n(r.base), 1), pct(lang, n(r.now), 1)]),
      },
    },
  };
}
