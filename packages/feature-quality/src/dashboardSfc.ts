/**
 * The SFC view as a figure (decision 0058), as the guide draws it: a dot per
 * cell on a common scale, the 90 % threshold and the 80–90 % band where the
 * fewest fixes turn a cell from 0 to 1; its table has each cell's missing,
 * margin and closeness. Pure, as `dashboard.ts`.
 */

import { dots, num, pct, type Figure, type Lang } from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const THRESHOLD = 0.9;
const BAND: [number, number] = [0.8, 0.9];

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));
const known = (v: unknown) => v !== null && v !== undefined;

/** The scale starts at 70 %, or lower, a tenth at a time, to hold every cell. */
function lowest(rows: Row[]): number {
  const ps = rows.filter((r) => known(r.p)).map((r) => n(r.p));
  return Math.min(0.7, Math.floor(Math.min(1, ...ps) * 10) / 10);
}

export function sfcView(lang: Lang, rows: Row[], tr: Texts): Figure {
  const f = (v: number) => pct(lang, v, 1);
  const passed = rows.filter((r) => r.passes === true).length;
  const sfc = known(rows[0].sfc) ? pct(lang, n(rows[0].sfc), 1) : "—";
  return {
    title: tr("sfc_view"),
    note: `${tr("sfcValue")}: ${num(lang, passed)} / ${num(lang, rows.length)} (${sfc}). ${tr("sfcNote")}`,
    live: true,
    plot: {
      svg: dots({
        rows: rows
          .filter((r) => known(r.p))
          .map((r) => ({ label: String(r.cell), value: n(r.p), ok: r.passes === true })),
        min: lowest(rows),
        max: 1,
        zone: BAND,
        zoneLabel: tr("sfcBand"),
        ref: { value: THRESHOLD, label: f(THRESHOLD) },
        fmt: f,
        okLabel: tr("sfcPasses"),
        failLabel: tr("sfcFails"),
        labelWidth: 200,
      }),
      legend: [
        { label: tr("sfcPasses"), color: "var(--viz-good)", kind: "dot", icon: "✓" },
        { label: tr("sfcFails"), color: "var(--viz-crit)", kind: "dot", icon: "✕" },
      ],
      table: {
        head: [
          tr("sfcCell"),
          tr("sfcRight"),
          tr("sfcActive"),
          tr("sfcShare"),
          tr("sfcState"),
          tr("sfcMissing"),
          tr("sfcMargin"),
          tr("sfcCloseness"),
        ],
        rows: rows.map((r) => [
          String(r.cell),
          num(lang, n(r.ok)),
          num(lang, n(r.total)),
          known(r.p) ? f(n(r.p)) : "—",
          r.passes === true ? "1" : "0",
          num(lang, n(r.missing)),
          num(lang, n(r.margin)),
          known(r.closeness) ? num(lang, n(r.closeness), 1) : "—",
        ]),
      },
    },
  };
}
