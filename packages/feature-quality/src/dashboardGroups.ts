/**
 * The groups' charts as figures (decision 0031), one per dimension: the p′
 * chart (Laney), the largest groups and those that changed most since the
 * previous cut. Pure, as `dashboard.ts`.
 */

import {
  bars,
  diverging,
  num,
  pct,
  scatter,
  type Figure,
  type Lang,
} from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));
const group = (tr: Texts, v: unknown) =>
  v === null || v === undefined || v === "" ? tr("emptyGroup") : String(v);
const icon = (signal: string) => (signal === "above" ? "✕ " : signal === "below" ? "✓ " : "");
const SIGNALS = ["in", "below", "above"] as const;
const COLORS = { in: "var(--viz-muted)", above: "var(--viz-crit)", below: "var(--viz-good)" };

/** The charts drawn once per dimension or variable: the column that splits them. */
export const SPLIT: Record<string, string> = {
  groups_p_chart: "dimension",
  largest_groups: "dimension",
  groups_changed: "dimension",
  outliers_by_group: "dimension",
  psi_by_decile: "variable",
  remediation_queues: "dimension",
};

/** The rows by their `key` column, in the order they come. */
export function splitBy(rows: Row[], key: string): [string, Row[]][] {
  const out = new Map<string, Row[]>();
  for (const r of rows) {
    const d = String(r[key]);
    const list = out.get(d);
    if (list) list.push(r);
    else out.set(d, [r]);
  }
  return [...out];
}

export function pChart(lang: Lang, rows: Row[], tr: Texts): Figure {
  const signal = (r: Row) => String(r.signal);
  const read = (r: Row) => tr(`signal.${signal(r)}`);
  const sorted = [...rows].sort((a, b) => n(a.records) - n(b.records));
  const yMax = Math.min(1, Math.max(0.05, ...rows.map((r) => Math.max(n(r.p), n(r.ucl)))) * 1.1);
  const pBar = n(rows[0]?.p_bar);
  return {
    title: tr("groups_p_chart"),
    note: tr("pChartNote"),
    live: true,
    plot: {
      svg: scatter({
        points: rows.map((r) => ({
          x: n(r.records),
          y: n(r.p),
          cls: signal(r),
          label: `${icon(signal(r))}${group(tr, r.grp)} · ${num(lang, n(r.records))} · ${read(r)}`,
          value: pct(lang, n(r.p), 1),
        })),
        classes: Object.fromEntries(SIGNALS.map((s) => [s, { color: COLORS[s] }])),
        order: [...SIGNALS],
        xMin: 0,
        xMax: Math.max(1, ...rows.map((r) => n(r.records))) * 1.05,
        yMin: 0,
        yMax,
        xFmt: (v) => num(lang, v),
        yFmt: (v) => pct(lang, v),
        xTitle: tr("records"),
        yTitle: tr("seriousShare"),
        curves: [
          { pts: sorted.map((r) => [n(r.records), n(r.ucl)] as [number, number]) },
          { pts: sorted.map((r) => [n(r.records), n(r.lcl)] as [number, number]) },
        ],
        hlines: [{ value: pBar, label: `p̄ = ${pct(lang, pBar, 1)}`, left: true }],
      }),
      legend: [
        ...SIGNALS.map((s) => ({
          label: tr(`signal.${s}`),
          color: COLORS[s],
          kind: "dot" as const,
          icon: icon(s).trim() || undefined,
        })),
        { label: tr("limits"), color: "var(--viz-ink2)", kind: "line" as const },
      ],
      table: {
        head: [tr("group"), tr("records"), tr("seriousShare"), tr("limits"), tr("reading")],
        rows: rows.map((r) => [
          group(tr, r.grp),
          num(lang, n(r.records)),
          pct(lang, n(r.p), 1),
          `${pct(lang, n(r.lcl), 1)} – ${pct(lang, n(r.ucl), 1)}`,
          read(r),
        ]),
      },
    },
  };
}

export function largestGroups(lang: Lang, rows: Row[], tr: Texts): Figure {
  const share = (v: unknown) => (v === null || v === undefined ? "—" : pct(lang, n(v), 1));
  return {
    title: tr("largest_groups"),
    live: true,
    plot: {
      svg: bars({
        rows: rows.map((r) => ({ label: group(tr, r.grp), value: n(r.records) })),
        max: Math.max(1, ...rows.map((r) => n(r.records))),
        fmt: (v) => num(lang, v),
      }),
      table: {
        head: [tr("group"), tr("records"), tr("seriousShare"), tr("icd")],
        rows: rows.map((r) => [
          group(tr, r.grp),
          num(lang, n(r.records)),
          share(r.serious_share),
          r.icd === null || r.icd === undefined ? "—" : num(lang, n(r.icd), 2),
        ]),
      },
    },
  };
}

export function groupsChanged(lang: Lang, rows: Row[], tr: Texts): Figure {
  const points = (v: number) => {
    const shown = num(lang, Math.abs(v) * 100, 1);
    const zero = shown === num(lang, 0, 1);
    return `${zero ? "" : v > 0 ? "+" : "-"}${shown} pp`;
  };
  const still = rows.every((r) => Math.abs(n(r.change)) < 0.0005);
  return {
    title: tr("groups_changed"),
    note: still ? tr("noChange") : undefined,
    live: true,
    plot: {
      svg: still
        ? ""
        : diverging({
            rows: rows.map((r) => ({ label: group(tr, r.grp), value: n(r.change) })),
            max: Math.max(0.01, ...rows.map((r) => Math.abs(n(r.change)))),
            fmt: points,
          }),
      table: {
        head: [tr("group"), tr("previousCut"), tr("thisCut"), tr("change")],
        rows: rows.map((r) => [
          group(tr, r.grp),
          pct(lang, n(r.before), 1),
          pct(lang, n(r.now), 1),
          points(n(r.change)),
        ]),
      },
    },
  };
}
