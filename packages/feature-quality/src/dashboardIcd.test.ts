import { describe, expect, it } from "vitest";
import { chartFigure, MOST_BARS } from "./dashboard";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

describe("the ICD's charts", () => {
  it("adds the IGCD to the trend once a cut has one", () => {
    const f = chartFigure(
      "trend_by_cut",
      [
        { cut: "2026-02-28", records: 90, failures: 9, conformity: 0.9, igcd: null },
        { cut: "2026-03-31", records: 100, failures: 40, conformity: 0.8, igcd: 0.825 },
      ],
      "en",
      tr,
    )!;
    expect(f.plot.table.head.at(-1)).toBe("[igcd]");
    expect(f.plot.table.rows.map((r) => r.at(-1))).toEqual(["—", "0.825"]);
    // A cut without an IGCD is a gap, never a fall to 0: here the IGCD is a
    // lone point beside the conformity's line.
    expect(f.plot.svg.match(/class="viz-line"/g)).toHaveLength(1);
    expect(f.plot.svg.match(/class="viz-dot"/g)).toHaveLength(2);
    expect(f.plot.legend?.map((l) => l.label)).toEqual(["[conformity]", "[igcd]"]);
  });

  it("draws the lowest fields and every bin of the distribution", () => {
    const rows = Array.from({ length: MOST_BARS + 2 }, (_, i) => ({ field: `F${i}`, icd: i / 20 }));
    const lowest = chartFigure("lowest_icd", rows, "en", tr)!;
    expect(lowest.plot.svg.match(/class="viz-mark"/g)).toHaveLength(MOST_BARS);
    expect(lowest.plot.table.rows[1]).toEqual(["F1", "0.05"]);

    const d = chartFigure(
      "icd_distribution",
      [
        { from_icd: 0.6, entities: 1 },
        { from_icd: 0.95, entities: 2 },
      ],
      "en",
      tr,
    )!;
    expect(d.plot.table.rows).toHaveLength(20);
    expect(d.plot.table.rows[12]).toEqual(["0.60 – 0.65", "1"]);
    expect(d.plot.table.rows[19]).toEqual(["0.95 – 1.00", "2"]);
    expect(d.plot.table.rows[0][1]).toBe("0");
  });

  it("names the bands, by segment and from the previous cut", () => {
    const seg = chartFigure(
      "bands_by_segment",
      [{ segment: "PJ", high: 1, medium: 1, low: 2 }],
      "en",
      tr,
    )!;
    expect(seg.plot.table.head).toEqual([
      "[segment]",
      "[bandName.high]",
      "[bandName.medium]",
      "[bandName.low]",
    ]);
    expect(seg.plot.table.rows[0]).toEqual(["PJ", "1 (25%)", "1 (25%)", "2 (50%)"]);

    const moved = chartFigure(
      "band_transitions",
      [
        { from_band: "high", to_band: "high", entities: 5 },
        { from_band: "medium", to_band: "low", entities: 2 },
      ],
      "en",
      tr,
    )!;
    expect(moved.plot.table.rows).toEqual([
      ["[bandName.high]", "5", "0", "0"],
      ["[bandName.medium]", "0", "0", "2"],
      ["[bandName.low]", "0", "0", "0"],
    ]);
  });
});

describe("the trend's edge cases", () => {
  it("joins the IGCD only over the cuts that have one and thins crowded dates", () => {
    const rows = Array.from({ length: 40 }, (_, i) => ({
      cut: `2026-${String(1 + Math.floor(i / 28)).padStart(2, "0")}-${String(1 + (i % 28)).padStart(2, "0")}`,
      records: 10,
      failures: 1,
      conformity: 0.9,
      igcd: i < 20 ? null : 0.8,
    }));
    const svg = chartFigure("trend_by_cut", rows, "en", tr)!.plot.svg;
    expect(svg.match(/class="viz-line"/g)).toHaveLength(2);
    const dates = svg.match(/class="viz-ax"[^>]*>2026-/g) ?? [];
    expect(dates.length).toBeLessThan(40);
    expect(svg).toContain(">2026-02-12<");
  });
});
