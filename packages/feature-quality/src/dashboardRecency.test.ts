import { describe, expect, it } from "vitest";
import { chartFigure } from "./dashboard";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

describe("the update recency", () => {
  it("draws each age band, the empty last, with its share of the records", () => {
    const rows = [
      { band: "b1", records: 0 },
      { band: "b2", records: 60 },
      { band: "b7", records: 30 },
      { band: "empty", records: 10 },
    ];
    const f = chartFigure("update_recency", rows, "en", tr)!;
    expect(f.title).toBe("[update_recency]");
    expect(f.plot.table.head).toEqual(["[age]", "[records]"]);
    expect(f.plot.table.rows).toEqual([
      ["[recencyBand.b1]", "0 (0.0%)"],
      ["[recencyBand.b2]", "60 (60.0%)"],
      ["[recencyBand.b7]", "30 (30.0%)"],
      ["[recencyBand.empty]", "10 (10.0%)"],
    ]);
  });
});
