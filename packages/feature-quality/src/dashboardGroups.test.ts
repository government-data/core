import { describe, expect, it } from "vitest";
import { chartFigure, chartHtml } from "./dashboard";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

// Synthetic offices and advisors.
const pRows = [
  {
    dimension: "office",
    grp: "O-1",
    records: 900,
    serious: 90,
    p: 0.1,
    p_bar: 0.12,
    lcl: 0.08,
    ucl: 0.16,
    signal: "in",
  },
  {
    dimension: "office",
    grp: "O-2",
    records: 100,
    serious: 30,
    p: 0.3,
    p_bar: 0.12,
    lcl: 0.0,
    ucl: 0.25,
    signal: "above",
  },
  {
    dimension: "advisor",
    grp: "A-1",
    records: 50,
    serious: 1,
    p: 0.02,
    p_bar: 0.05,
    lcl: 0.03,
    ucl: 0.2,
    signal: "below",
  },
];

describe("the groups' charts", () => {
  it("draws one figure per dimension, named after it", () => {
    const html = chartHtml("groups_p_chart", pRows, "en", tr);
    expect(html.match(/<figure/g)).toHaveLength(2);
    expect(html).toContain("[groups_p_chart] · office");
    expect(html).toContain("[groups_p_chart] · advisor");
  });

  it("reads each group of the p′ chart against its limits", () => {
    const f = chartFigure("groups_p_chart", pRows.slice(0, 2), "en", tr)!;
    expect(f.plot.table.rows).toEqual([
      ["O-1", "900", "10.0%", "8.0% – 16.0%", "[signal.in]"],
      ["O-2", "100", "30.0%", "0.0% – 25.0%", "[signal.above]"],
    ]);
    expect(f.plot.svg).toContain("p̄ = 12.0%");
  });

  it("names the empty group and signs each change in points", () => {
    const largest = chartFigure(
      "largest_groups",
      [{ dimension: "office", grp: "", records: 40, serious: 4, serious_share: 0.1, icd: null }],
      "en",
      tr,
    )!;
    expect(largest.plot.table.rows).toEqual([["[emptyGroup]", "40", "10.0%", "—"]]);
    const changed = chartFigure(
      "groups_changed",
      [
        { dimension: "office", grp: "O-2", before: 0.1, now: 0.3, change: 0.2 },
        { dimension: "office", grp: "O-1", before: 0.15, now: 0.1, change: -0.05 },
      ],
      "en",
      tr,
    )!;
    expect(changed.plot.table.rows.map((r) => r[3])).toEqual(["+20.0 pp", "-5.0 pp"]);
  });
});

describe("the groups' edge cases", () => {
  it("says when no group changed, without signing a zero", () => {
    const f = chartFigure(
      "groups_changed",
      [{ dimension: "office", grp: "O-1", before: 0.1, now: 0.10001, change: 0.00001 }],
      "en",
      tr,
    )!;
    expect(f.note).toBe("[noChange]");
    expect(f.plot.svg).toBe("");
    expect(f.plot.table.rows[0][3]).toBe("0.0 pp");
  });

  it("cuts a long group name in the drawing, whole in its table", () => {
    const name = "an_advisor_named_at_great_length_beyond_any_label_width";
    const f = chartFigure(
      "largest_groups",
      [{ dimension: "advisor", grp: name, records: 5, serious: 0, serious_share: 0, icd: 1 }],
      "en",
      tr,
    )!;
    expect(f.plot.svg).toContain("…");
    expect(f.plot.svg).toContain(`data-l="${name}"`);
    expect(f.plot.table.rows[0][0]).toBe(name);
  });
});
