import { describe, expect, it } from "vitest";
import type { FindingOut } from "./api";
import { isStyle, marks, ruleLine, tidies } from "./editor";

const finding = (line: number | null, column: number | null, severity = "error"): FindingOut => ({
  code: "QR003",
  severity,
  rule: null,
  line,
  column,
  message: "m",
});

const TEXT = "dataset: d\nrules:\n  - id: R-01\n    field: Nope\n";

describe("marks", () => {
  it("marks from the column to the end of the line", () => {
    const [m] = marks(TEXT, [finding(4, 12)]);
    expect(TEXT.slice(m.from, m.to)).toBe("Nope");
    expect(m).toMatchObject({ severity: "error", message: "QR003: m" });
  });

  it("puts a finding with no place on the first line", () => {
    const [m] = marks(TEXT, [finding(null, null, "warning")]);
    expect(TEXT.slice(m.from, m.to)).toBe("dataset: d");
    expect(m.severity).toBe("warning");
  });

  it("clamps places past the text", () => {
    const [m] = marks("a: 1", [finding(9, 40)]);
    expect([m.from, m.to]).toEqual([4, 4]);
  });
});

describe("ruleLine", () => {
  it("finds the line a rule starts on, quoted or not", () => {
    expect(ruleLine(TEXT, "R-01")).toBe(3);
    expect(ruleLine('rules:\n  - id: "P-07"  # note\n', "P-07")).toBe(2);
    expect(ruleLine(TEXT, "R-1")).toBeUndefined();
  });
});

describe("isStyle", () => {
  it("tells the style findings (0041) from the rest", () => {
    expect(["QR200", "QR203", "QR205", "QR210"].every(isStyle)).toBe(true);
    expect(["QR003", "QR020", "QR103", "QR206"].some(isStyle)).toBe(false);
  });
});

describe("tidies", () => {
  it("knows what Tidy takes away", () => {
    expect(["QR201", "QR203", "QR204", "QR206", "QR208", "QR209"].every(tidies)).toBe(true);
    expect(["QR001", "QR200", "QR207"].some(tidies)).toBe(false);
  });
});
