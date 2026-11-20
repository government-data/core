/**
 * The ICD's charts as figures (decision 0057): the fields with the lowest
 * ICD, the entities' distribution, their bands by segment, and how they
 * moved between bands since the previous cut. Pure, as `dashboard.ts`.
 */

import {
  bars,
  columns,
  heatmap,
  num,
  pct,
  stacked,
  type Figure,
  type Lang,
} from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const BANDS = ["high", "medium", "low"] as const;
const COLORS = ["var(--viz-good)", "var(--viz-warn)", "var(--viz-crit)"];
const INKS = ["#ffffff", "#0b0b0b", "#ffffff"];
/** Bins of 0.05: the first names 0.00, the last 0.95 (and 1.0). */
const BINS = 20;

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));
const icd = (lang: Lang, v: unknown) => (v === null || v === undefined ? "—" : num(lang, n(v), 2));
const bandName = (tr: Texts, band: unknown) => tr(`bandName.${String(band)}`);

export function lowestIcd(lang: Lang, rows: Row[], tr: Texts, most: number): Figure {
  return {
    title: tr("lowest_icd"),
    live: true,
    plot: {
      svg: bars({
        rows: rows.slice(0, most).map((r) => ({ label: String(r.field), value: n(r.icd) })),
        max: 1,
        fmt: (v) => num(lang, v, 2),
      }),
      table: {
        head: [tr("field"), tr("icd")],
        rows: rows.map((r) => [String(r.field), icd(lang, r.icd)]),
      },
    },
  };
}

export function distribution(lang: Lang, rows: Row[], tr: Texts): Figure {
  const counts = Array.from({ length: BINS }, () => 0);
  for (const r of rows)
    counts[Math.min(BINS - 1, Math.round(n(r.from_icd) * BINS))] += n(r.entities);
  const from = (i: number) => num(lang, i / BINS, 2);
  return {
    title: tr("icd_distribution"),
    live: true,
    plot: {
      svg: columns({
        cats: counts.map((_, i) => from(i)),
        series: [{ label: tr("entities"), color: "var(--viz-s1)", values: counts }],
        yMax: Math.max(1, ...counts),
        yFmt: (v) => num(lang, v),
        valFmt: (v) => num(lang, v),
        labelEvery: 2,
        edgeLabels: true,
        vrefs: [
          { at: 15, label: num(lang, 0.75, 2) },
          { at: 18, label: num(lang, 0.9, 2) },
        ],
      }),
      table: {
        head: [tr("icdBin"), tr("entities")],
        rows: counts.map((c, i) => [`${from(i)} – ${num(lang, (i + 1) / BINS, 2)}`, num(lang, c)]),
      },
    },
  };
}

export function bandsBySegment(lang: Lang, rows: Row[], tr: Texts, most: number): Figure {
  const series = BANDS.map((b, i) => ({ label: bandName(tr, b), color: COLORS[i], ink: INKS[i] }));
  const segment = (r: Row) =>
    r.segment === null || r.segment === undefined ? "—" : String(r.segment);
  const parts = (r: Row) => BANDS.map((b) => n(r[b]));
  const size = (r: Row) => parts(r).reduce((a, b) => a + b, 0);
  // The largest segments drawn, every one in the table.
  rows = [...rows].sort((a, b) => size(b) - size(a));
  const share = (v: number, t: number) => pct(lang, t ? v / t : 0);
  return {
    title: tr("bands_by_segment"),
    live: true,
    plot: {
      svg: stacked({
        rows: rows.slice(0, most).map((r) => ({ label: segment(r), parts: parts(r) })),
        series,
        percent: true,
        fmt: share,
        labelWidth: 130,
      }),
      table: {
        head: [tr("segment"), ...series.map((s) => s.label)],
        rows: rows.map((r) => {
          const t = parts(r).reduce((a, b) => a + b, 0);
          return [segment(r), ...parts(r).map((p) => `${num(lang, p)} (${share(p, t)})`)];
        }),
      },
    },
  };
}

export function transitions(lang: Lang, rows: Row[], tr: Texts): Figure {
  const names = BANDS.map((b) => bandName(tr, b));
  const values = BANDS.map((from) =>
    BANDS.map((to) => n(rows.find((r) => r.from_band === from && r.to_band === to)?.entities)),
  );
  const f = (v: number) => num(lang, v);
  return {
    title: tr("band_transitions"),
    live: true,
    plot: {
      svg: heatmap({
        rows: names,
        cols: names,
        values,
        rowTitle: tr("previousCut"),
        colTitle: tr("thisCut"),
        fmt: f,
      }),
      table: {
        head: [tr("transitionHead"), ...names],
        rows: names.map((name, i) => [name, ...values[i].map(f)]),
      },
    },
  };
}
