/**
 * The time per stage as a figure (decision 0031, the guide's formula 9):
 * how long each stage of a cut's evaluation took per million
 * record-attributes, the previous cut's beside it, so the base's growth
 * does not read as slowness. Pure, as `dashboard.ts`.
 */

import { bars, num, type Figure, type Lang } from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));
const given = (v: unknown) => v !== null && v !== undefined;

/** Each stage as a bar (its names are too long for columns): its ms per
 * million record-attributes, or its ms when the cut has no volume; the
 * previous cut's in the table. */
export function timeByStage(lang: Lang, rows: Row[], tr: Texts): Figure {
  const cats = rows.map((r) => tr(`stageName.${String(r.stage)}`));
  const ms = (v: unknown) => (given(v) ? `${num(lang, n(v))} ms` : "—");
  const rate = (v: unknown) => (given(v) ? num(lang, n(v), 2) : "—");
  const rated = rows.some((r) => given(r.per_million));
  const value = (r: Row) => n(rated ? r.per_million : r.ms);
  return {
    title: tr("time_by_stage"),
    note: tr(rated ? "stageNote" : "stageNoteMs"),
    live: true,
    plot: {
      svg: bars({
        rows: rows.map((r, i) => ({ label: cats[i], value: value(r) })),
        max: Math.max(1, ...rows.map(value)),
        fmt: (v) => num(lang, v, rated ? 2 : 0),
        labelWidth: 170,
      }),
      table: {
        head: [
          tr("stage"),
          tr("stageMs"),
          tr("perMillion"),
          tr("previousMs"),
          tr("previousPerMillion"),
        ],
        rows: rows.map((r, i) => [
          cats[i],
          ms(r.ms),
          rate(r.per_million),
          ms(r.previous_ms),
          rate(r.previous_per_million),
        ]),
      },
    },
  };
}
