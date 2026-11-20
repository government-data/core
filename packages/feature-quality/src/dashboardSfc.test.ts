import { describe, expect, it } from "vitest";
import { chartFigure } from "./dashboard";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

// Synthetic cells of 100 active records, as `sfc_view` gives them.
const cell = (name: string, ok: number, passes: boolean, closeness: number) => ({
  cell: name,
  ok,
  total: 100,
  p: ok / 100,
  passes,
  missing: Math.max(0, 90 - ok),
  margin: Math.max(0, ok - 90),
  closeness,
  sfc: 0.5,
});

describe("the SFC view", () => {
  it("draws a dot per cell with the threshold and tells the share passing", () => {
    const rows = [
      cell("Name", 99, true, 0.2),
      cell("Phone", 91, true, 0.5),
      cell("Email", 85, false, 1.0),
      cell("City", 60, false, 0.3),
    ];
    const f = chartFigure("sfc_view", rows, "en", tr)!;
    expect(f.title).toBe("[sfc_view]");
    expect(f.note).toContain("[sfcValue]: 2 / 4 (50.0%)");
    expect(f.plot.svg.match(/class="viz-dot"/g)).toHaveLength(4);
    expect(f.plot.svg).toContain('class="viz-ref"');
    // City at 60 % takes the scale down to 60 %.
    expect(f.plot.svg).toContain(">60.0%<");
    expect(f.plot.table.rows[2]).toEqual(["Email", "85", "100", "85.0%", "0", "5", "0", "1.0"]);
    expect(f.plot.table.rows[1]).toEqual(["Phone", "91", "100", "91.0%", "1", "0", "1", "0.5"]);
  });

  it("keeps a cell without active records in its table, off the scale", () => {
    const empty = { ...cell("Phone", 0, false, 0), total: 0, p: null, closeness: null, missing: 0 };
    const f = chartFigure("sfc_view", [empty], "es", tr)!;
    expect(f.plot.svg.match(/class="viz-dot"/g)).toBeNull();
    expect(f.plot.table.rows[0]).toEqual(["Phone", "0", "0", "—", "0", "0", "0", "—"]);
  });
});
