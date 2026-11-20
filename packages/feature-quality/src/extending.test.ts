import { describe, expect, it } from "vitest";
import type { RuleView } from "./api";
import { extendsNotes } from "./extending";

const rule = (id: string, extend?: { named: string; line: number }): RuleView =>
  ({
    id,
    field: "A",
    reads: ["A"],
    pattern: "required",
    kind: "",
    dimension: "",
    severity: "",
    applies: "every record",
    fails: "A is empty",
    extends: extend ?? null,
  }) as RuleView;

describe("extendsNotes", () => {
  it("says at each extends line what the rule does, once a line", () => {
    const notes = extendsNotes([
      rule("R-01.A", { named: "filled", line: 5 }),
      rule("R-01.B", { named: "filled", line: 5 }),
      rule("R-02"),
    ]);
    expect(notes).toHaveLength(1);
    expect(notes[0]!.line).toBe(5);
    expect(notes[0]!.text).toContain("filled");
    expect(notes[0]!.text).toContain("A is empty");
    expect(notes[0]!.text.split("\n")).toHaveLength(1);
  });
});
