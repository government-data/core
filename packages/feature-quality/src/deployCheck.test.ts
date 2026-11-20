import { describe, expect, it } from "vitest";
import type { CheckOut } from "./api";
import { review } from "./deployCheck";

const rule = (id: string, pattern = "required") =>
  ({
    id,
    field: "id",
    reads: ["id"],
    pattern,
    kind: "",
    dimension: "",
    severity: "",
    applies: "",
    fails: "",
  }) as unknown as CheckOut["rules"][number];
const out = (rules: CheckOut["rules"], findings: CheckOut["findings"] = []): CheckOut => ({
  rules,
  findings,
  fields: [],
});

describe("review", () => {
  it("says which rules a deploy adds, removes or changes, and what the linter found", () => {
    const before = out([rule("a"), rule("b"), rule("c")]);
    const after = out([rule("a"), rule("b", "unique"), rule("d")], [
      { code: "QR001", severity: "error", message: "x", line: 3 },
      { code: "QR040", severity: "warning", message: "y", line: 5 },
    ] as CheckOut["findings"]);
    const r = review(before, after);
    expect([r.added, r.removed, r.changed]).toEqual([["d"], ["c"], ["b"]]);
    expect([r.errors.length, r.warnings.length]).toEqual([1, 1]);
  });

  it("takes every rule as added when main has no file", () => {
    expect(review(undefined, out([rule("a")])).added).toEqual(["a"]);
  });
});
