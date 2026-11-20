import { describe, expect, it } from "vitest";
import { chartFigure, chartHtml } from "./dashboard";
import { psiReading } from "./dashboardDistribution";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

describe("the distributions' charts", () => {
  it("draws the outliers of each dimension with their share", () => {
    const rows = [
      {
        variable: "Income",
        dimension: "office",
        grp: "O-1",
        records: 200,
        low: 2,
        high: 8,
        share: 0.05,
      },
      { variable: "Income", dimension: "advisor", grp: "", records: 10, low: 0, high: 0, share: 0 },
    ];
    expect(chartHtml("outliers_by_group", rows, "en", tr).match(/<figure/g)).toHaveLength(2);
    const f = chartFigure("outliers_by_group", rows.slice(0, 1), "en", tr)!;
    expect(f.note).toBe("Income · [outliersNote]");
    expect(f.plot.table.rows).toEqual([["O-1", "200", "2", "8", "5.0%"]]);
  });

  it("reads the PSI of each variable against the guide's thresholds", () => {
    const rows = Array.from({ length: 10 }, (_, i) => ({
      variable: "Income",
      decile: i + 1,
      base: 0.1,
      now: i === 0 ? 0.3 : 0.0778,
      psi: 0.2843,
    }));
    const f = chartFigure("psi_by_decile", rows, "en", tr)!;
    expect(f.note).toBe("PSI = 0.284 · [psiReading.significant]");
    expect(f.plot.table.rows[0]).toEqual(["1", "10.0%", "30.0%"]);
    expect([psiReading(0.05), psiReading(0.25), psiReading(0.26)]).toEqual([
      "stable",
      "moderate",
      "significant",
    ]);
  });
});

describe("the outliers' edge cases", () => {
  it("draws only the groups with an outlier, and says when none has", () => {
    const rows = [
      {
        variable: "Income",
        dimension: "office",
        grp: "O-1",
        records: 10,
        low: 0,
        high: 1,
        share: 0.1,
      },
      {
        variable: "Income",
        dimension: "office",
        grp: "O-2",
        records: 10,
        low: 0,
        high: 0,
        share: 0,
      },
    ];
    const some = chartFigure("outliers_by_group", rows, "en", tr)!;
    expect(some.plot.svg).not.toContain('data-l="O-2');
    expect(some.plot.table.rows).toHaveLength(2);
    const none = chartFigure("outliers_by_group", rows.slice(1), "en", tr)!;
    expect(none.note).toBe("Income · [noOutliers]");
  });
});
