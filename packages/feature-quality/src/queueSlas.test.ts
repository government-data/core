import { describe, expect, it } from "vitest";
import { daysOf, formOf, MAX_DAYS, slasOf } from "./queueSlas";

describe("remediation SLAs in a form", () => {
  const slas = [
    { dimension: "Office", group: null, days: 15 },
    { dimension: "Office", group: "North", days: 7 },
    { dimension: "Office", group: "", days: 60 },
    { dimension: "Gone", group: null, days: 9 },
  ];

  it("fills each dimension with its default and overrides, blank when none", () => {
    expect(formOf(["Advisor", "Office"], slas)).toEqual([
      { dimension: "Advisor", days: "", overrides: [] },
      {
        dimension: "Office",
        days: "15",
        overrides: [
          { group: "North", days: "7" },
          { group: "", days: "60" },
        ],
      },
    ]);
  });

  it("gives back what it read, without the dimensions no longer in the mapping", () => {
    expect(slasOf(formOf(["Advisor", "Office"], slas))).toEqual({ slas: slas.slice(0, 3) });
  });

  it("takes whole days from 1 to the most", () => {
    expect(daysOf(" 30 ")).toBe(30);
    expect(daysOf(String(MAX_DAYS))).toBe(MAX_DAYS);
    for (const bad of ["0", "1.5", "-3", "abc", "", String(MAX_DAYS + 1)]) {
      expect(daysOf(bad)).toBeUndefined();
    }
  });

  it("refuses bad days and a group overridden twice", () => {
    const form = formOf(["Office"], slas);
    expect(slasOf([{ ...form[0]!, days: "0" }])).toEqual({ problem: "days" });
    expect(slasOf([{ ...form[0]!, overrides: [{ group: "North", days: "" }] }])).toEqual({
      problem: "days",
    });
    const twice = [
      { group: "North", days: "3" },
      { group: "North", days: "4" },
    ];
    expect(slasOf([{ ...form[0]!, overrides: twice }])).toEqual({ problem: "duplicate" });
  });
});
