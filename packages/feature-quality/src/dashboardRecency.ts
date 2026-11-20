/**
 * The update recency as a figure (decision 0031): the records of a cut by
 * how long before it they were last updated, the empty ones last. Pure, as
 * `dashboard.ts`.
 */

import { bars, num, pct, type Figure, type Lang } from "@knime-orchestador/charts";
import type { Row, Texts } from "./dashboard";

const n = (v: unknown) => (typeof v === "number" ? v : Number(v ?? 0));

export function recency(lang: Lang, rows: Row[], tr: Texts): Figure {
  const cats = rows.map((r) => tr(`recencyBand.${String(r.band)}`));
  const values = rows.map((r) => n(r.records));
  const total = values.reduce((a, b) => a + b, 0);
  const share = (v: number) => pct(lang, total ? v / total : 0, 1);
  return {
    title: tr("update_recency"),
    live: true,
    plot: {
      svg: bars({
        rows: values.map((v, i) => ({ label: cats[i], value: v })),
        max: Math.max(1, ...values),
        fmt: share,
        labelWidth: 150,
      }),
      table: {
        head: [tr("age"), tr("records")],
        rows: values.map((v, i) => [cats[i], `${num(lang, v)} (${share(v)})`]),
      },
    },
  };
}
