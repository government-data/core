import { describe, expect, it } from "vitest";
import { chartFigure } from "./dashboard";

// Keys as texts, so the test reads what each place asks for.
const tr = (key: string) => `[${key}]`;

describe("the time per stage", () => {
  it("shows each stage's time and its rate per million, the previous cut's beside it", () => {
    const rows = [
      { stage: "load", ms: 1200, per_million: 4, previous_ms: 900, previous_per_million: 3.5 },
      { stage: "rules", ms: 3000, per_million: 10, previous_ms: null, previous_per_million: null },
    ];
    const f = chartFigure("time_by_stage", rows, "en", tr)!;
    expect(f.title).toBe("[time_by_stage]");
    expect(f.note).toBe("[stageNote]");
    expect(f.plot.svg).toContain('data-l="[stageName.rules]"');
    expect(f.plot.table.rows).toEqual([
      ["[stageName.load]", "1,200 ms", "4.00", "900 ms", "3.50"],
      ["[stageName.rules]", "3,000 ms", "10.00", "—", "—"],
    ]);
  });

  it("draws each stage's ms when the cut has no volume to rate them by", () => {
    const rows = [{ stage: "load", ms: 5, per_million: null, previous_ms: null }];
    const f = chartFigure("time_by_stage", rows, "en", tr)!;
    expect(f.note).toBe("[stageNoteMs]");
    expect(f.plot.svg).toContain('data-v="5"');
  });
});
